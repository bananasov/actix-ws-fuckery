use std::{
    str::FromStr,
    sync::Arc,
    time::{Duration, Instant},
};

use actix_web::{HttpRequest, HttpResponse, error::ErrorBadRequest, get, rt::time, web};
use actix_ws::{AggregatedMessage, Session};
use bytestring::ByteString;
use dashmap::DashMap;
use tokio::sync::Mutex;
use tracing::instrument;
use uuid::Uuid;

const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(5);
const CLIENT_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Clone)]
pub struct WebSocketServer {
    inner: Arc<Mutex<WebSocketServerInner>>,
}

#[derive(Clone)]
pub struct WebSocketServerInner {
    sessions: DashMap<Uuid, Session>,
}

impl WebSocketServer {
    pub fn new() -> Self {
        let inner = WebSocketServerInner {
            sessions: DashMap::new(),
        };

        Self {
            inner: Arc::new(Mutex::new(inner)),
        }
    }

    async fn insert_session(&self, uuid: Uuid, session: Session) {
        self.inner.lock().await.sessions.insert(uuid, session);
    }

    async fn cleanup_session(&self, uuid: &Uuid) {
        tracing::info!("Cleaning up session {uuid}");
        self.inner.lock().await.sessions.remove(uuid);
    }

    /// Broadcast a message to all connected clients
    pub async fn broadcast(&self, msg: impl Into<ByteString>) {
        let msg = msg.into();

        let inner = self.inner.lock().await;

        // TODO: Do this in parallel, maybe using `FuturesUnordered`
        for mut entry in inner.sessions.iter_mut() {
            let msg = msg.clone();
            let session = entry.value_mut();
            tracing::info!("Sending msg: {msg}");

            let _ = session
                .text(msg)
                .await
                .map_err(|_| tracing::debug!("Dropping session"));
        }
    }
}

#[get("/gateway/{token}")]
#[instrument(skip_all, fields(token = *token), level = "debug")]
pub async fn ws_handler(
    req: HttpRequest,
    body: web::Payload,
    token: web::Path<String>,
    server: web::Data<WebSocketServer>,
) -> Result<HttpResponse, actix_web::Error> {
    let token = token.into_inner();
    let (response, mut session, stream) = actix_ws::handle(&req, body)?;

    let mut stream = stream
        .max_frame_size(64 * 1024)
        .aggregate_continuations()
        .max_continuation_size(2 * 1024 * 1024);

    let uuid = Uuid::from_str(&token).map_err(|err| ErrorBadRequest(err))?;
    server.insert_session(uuid, session.clone()).await;
    tracing::info!("Inserted new session");

    let alive = Arc::new(Mutex::new(Instant::now()));
    let mut session2 = session.clone();
    let alive2 = alive.clone();

    // Heartbeat stuff
    actix_web::rt::spawn(async move {
        let mut interval = time::interval(HEARTBEAT_INTERVAL);

        loop {
            interval.tick().await;
            if session2.ping(b"").await.is_err() {
                break;
            }

            if Instant::now().duration_since(*alive2.lock().await) > CLIENT_TIMEOUT {
                let _ = session2.close(None).await;
                break;
            }
        }
    });

    // Message handling
    actix_web::rt::spawn(async move {
        while let Some(Ok(msg)) = stream.recv().await {
            match msg {
                AggregatedMessage::Ping(bytes) => {
                    if session.pong(&bytes).await.is_err() {
                        tracing::error!("Failed to send pong back to session");
                        return;
                    }
                }

                AggregatedMessage::Text(string) => {
                    tracing::info!("Relaying text, {string}");

                    if session.text(string).await.is_err() {
                        tracing::error!("Failed to send text back to session");
                        return;
                    }
                }

                AggregatedMessage::Close(reason) => {
                    let _ = session.close(reason).await;

                    tracing::info!("Got close, cleaning up");
                    server.cleanup_session(&uuid).await;

                    return;
                }

                AggregatedMessage::Pong(_) => {
                    *alive.lock().await = Instant::now();
                }

                _ => (), // Binary data is just ignored
            }
        }

        let _ = session.close(None).await;
        server.cleanup_session(&uuid).await;
    });

    Ok(response)
}
