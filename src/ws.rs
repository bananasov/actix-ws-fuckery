use std::{
    str::FromStr,
    sync::Arc,
    time::{Duration, Instant},
};

use actix_web::{HttpRequest, HttpResponse, error::ErrorBadRequest, get, post, rt::time, web};
use actix_ws::{AggregatedMessage, Session};
use anyhow::anyhow;
use bytestring::ByteString;
use dashmap::DashMap;
use tokio::sync::Mutex;
use tracing::instrument;
use uuid::Uuid;

use crate::models::websocket::{
    WebSocketStartConnectionBody, WebSocketStartResponse, WebSocketTokenData,
};

const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(5);
const CLIENT_TIMEOUT: Duration = Duration::from_secs(10);
const TOKEN_EXPIRATION: Duration = Duration::from_secs(30);

#[derive(Clone)]
pub struct WebSocketServer {
    inner: Arc<Mutex<WebSocketServerInner>>,
}

#[derive(Clone)]
pub struct WebSocketServerInner {
    sessions: DashMap<Uuid, Session>,
    pending_tokens: DashMap<Uuid, WebSocketTokenData>,
}

impl WebSocketServer {
    pub fn new() -> Self {
        let inner = WebSocketServerInner {
            sessions: DashMap::new(),
            pending_tokens: DashMap::new(),
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

    #[instrument(skip_all, fields(address = token_data.address))]
    pub async fn obtain_token(&self, token_data: WebSocketTokenData) -> Uuid {
        let inner = self.inner.lock().await;
        let inner_clone = self.inner.clone();

        let uuid = Uuid::new_v4();

        let _ = inner.pending_tokens.insert(uuid, token_data);
        tracing::debug!("Inserting token {uuid} into cache");

        actix_web::rt::spawn(async move {
            tokio::time::sleep(TOKEN_EXPIRATION).await;

            let inner_mutex = inner_clone.lock().await;
            if inner_mutex.pending_tokens.contains_key(&uuid) {
                tracing::info!("Removing token {uuid}, expired");
                inner_mutex.pending_tokens.remove(&uuid);
            }
        });

        uuid
    }

    pub async fn use_token(&self, uuid: &Uuid) -> Result<WebSocketTokenData, anyhow::Error> {
        let inner = self.inner.lock().await;

        tracing::debug!("Using token {uuid}");
        let token = inner
            .pending_tokens
            .get(uuid)
            .ok_or_else(|| anyhow!("Expected token to exist"))?;
        let token = token.clone(); // I don't like it, yolo

        Ok(token)
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

#[post("/ws/start")]
pub async fn start_ws(
    server: web::Data<WebSocketServer>,
    details: Option<web::Json<WebSocketStartConnectionBody>>,
) -> Result<HttpResponse, actix_web::Error> {
    let private_key = details.map(|details| details.private_key.clone()); // I do not like this, we ball. Blame serde-json.

    let token = match private_key {
        Some(private_key) => {
            let address = String::from("dummyaddr");
            let token_data = WebSocketTokenData::new(address, Some(private_key));

            server.obtain_token(token_data).await
        }
        None => {
            let token_data = WebSocketTokenData::new("guest".into(), None);

            server.obtain_token(token_data).await
        }
    };

    let response = WebSocketStartResponse {
        ok: true,
        url: format!("ws://127.0.0.1:8080/gateway/{token}"),
        expires: 30,
    };

    Ok(HttpResponse::Ok().json(response))
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

    let token = Uuid::from_str(&token).map_err(|err| ErrorBadRequest(err))?;
    let data = server
        .use_token(&token)
        .await
        .expect("Token does not exist, sad");

    server.insert_session(token, session.clone()).await;
    tracing::info!("Inserted new session (address: {})", data.address);

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
                    server.cleanup_session(&token).await;

                    return;
                }

                AggregatedMessage::Pong(_) => {
                    *alive.lock().await = Instant::now();
                }

                _ => (), // Binary data is just ignored
            }
        }

        let _ = session.close(None).await;
        server.cleanup_session(&token).await;
    });

    Ok(response)
}
