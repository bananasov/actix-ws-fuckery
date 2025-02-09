use std::{
    str::FromStr,
    sync::Arc,
    time::{Duration, Instant},
};

use actix_web::{HttpRequest, HttpResponse, error::ErrorBadRequest, get, post, rt::time, web};
use actix_ws::{AggregatedMessage, Session};
use anyhow::anyhow;
use bytestring::ByteString;
use dashmap::{DashMap, DashSet};
use futures::{StreamExt, stream::FuturesUnordered};
use tokio::sync::Mutex;
use tracing::instrument;
use uuid::Uuid;

use crate::models::websocket::{
    WebSocketSessionData, WebSocketTokenData,
    messages::{WebSocketMessage, WebSocketMessageInner, WebSocketMessageResponse},
};
use crate::models::websocket::{
    WebSocketStartConnectionBody, WebSocketStartResponse, WebSocketSubscriptionType,
};

const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(5);
const CLIENT_TIMEOUT: Duration = Duration::from_secs(10);
const TOKEN_EXPIRATION: Duration = Duration::from_secs(30);

#[derive(Clone)]
pub struct WebSocketServer {
    inner: Arc<Mutex<WebSocketServerInner>>,
}

#[derive(Clone, Default)]
pub struct WebSocketServerInner {
    sessions: DashMap<Uuid, WebSocketSessionData>,
    pending_tokens: DashMap<Uuid, WebSocketTokenData>,
}

impl WebSocketServer {
    pub fn new() -> Self {
        let inner = WebSocketServerInner::default();

        Self {
            inner: Arc::new(Mutex::new(inner)),
        }
    }

    pub async fn insert_session(&self, uuid: Uuid, session: Session, data: WebSocketTokenData) {
        let subscriptions = DashSet::from_iter(vec![
            WebSocketSubscriptionType::OwnTransactions,
            WebSocketSubscriptionType::Blocks,
        ]);

        let session_data = WebSocketSessionData {
            address: data.address,
            private_key: data.private_key,
            session,
            subscriptions,
        };

        self.inner.lock().await.sessions.insert(uuid, session_data);
    }

    pub async fn cleanup_session(&self, uuid: &Uuid) {
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
            time::sleep(TOKEN_EXPIRATION).await;

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

        let (_uuid, token) = inner
            .pending_tokens
            .remove(uuid)
            .ok_or_else(|| anyhow!("Expected token to exist"))?; // TODO: Use proper error messages instead of anyhow

        Ok(token)
    }

    pub async fn subscribe_to_event(&self, uuid: &Uuid, event: WebSocketSubscriptionType) {
        let inner = self.inner.lock().await;

        let entry = inner.sessions.get_mut(uuid);
        if let Some(data) = entry {
            tracing::info!("Session {uuid} subscribed to event {event}");
            data.subscriptions.insert(event);
        } else {
            tracing::info!("Tried to subscribe to event {event} but found a non-existent session");
        }
    }

    pub async fn unsubscribe_from_event(&self, uuid: &Uuid, event: &WebSocketSubscriptionType) {
        let inner = self.inner.lock().await;

        let entry = inner.sessions.get_mut(uuid);
        if let Some(data) = entry {
            tracing::info!("Session {uuid} unsubscribed from event {event}");
            data.subscriptions.remove(event);
        }
    }

    pub async fn get_subscription_list(&self, uuid: &Uuid) -> Vec<WebSocketSubscriptionType> {
        let inner = self.inner.lock().await;

        let entry = inner.sessions.get(uuid);
        if let Some(data) = entry {
            let subscriptions: Vec<WebSocketSubscriptionType> =
                data.subscriptions.iter().map(|x| x.clone()).collect(); // not my fav piece of code but it works
            return subscriptions;
        }

        Vec::new()
    }

    /// Broadcast a message to all connected clients
    pub async fn broadcast(&self, msg: impl Into<ByteString>) {
        let msg = msg.into();

        let inner = self.inner.lock().await;
        let mut futures = FuturesUnordered::new();

        for mut entry in inner.sessions.iter_mut() {
            let msg = msg.clone();
            tracing::info!("Sending msg: {msg}");

            futures.push(async move {
                let session_data = entry.value_mut();
                session_data.session.text(msg).await
            });
        }

        while let Some(result) = futures.next().await {
            if result.is_err() {
                tracing::warn!("Got an unexpected closed session");
            }
        }
    }
}

#[post("/ws/start")]
pub async fn start_ws(
    server: web::Data<WebSocketServer>,
    details: Option<web::Json<WebSocketStartConnectionBody>>,
) -> Result<HttpResponse, actix_web::Error> {
    let details = details.map(|d| d.into_inner()).unwrap_or_default(); // I do not like this, we ball. Blame serde-json.

    let token = match details.private_key {
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
    let server = server.into_inner(); // guh but okay
    let (response, mut session, stream) = actix_ws::handle(&req, body)?;

    let mut stream = stream
        .max_frame_size(64 * 1024)
        .aggregate_continuations()
        .max_continuation_size(2 * 1024 * 1024);

    let token = Uuid::from_str(&token).map_err(ErrorBadRequest)?;
    let data = server
        .use_token(&token)
        .await
        .expect("Token does not exist, sad");

    tracing::info!("Inserting new session (address: {})", data.address);
    server.insert_session(token, session.clone(), data).await;

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
                    let msg: WebSocketMessage =
                        serde_json::from_str(&string).expect("wtf happened vro");
                    tracing::info!("{:?}", msg);

                    handle_websocket_message(&mut session, &token, &server, msg).await;
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

async fn handle_websocket_message(
    session: &mut Session,
    uuid: &Uuid,
    server: &WebSocketServer,
    message: WebSocketMessage,
) {
    match message.r#type {
        WebSocketMessageInner::Hello { motd: _ } => {} // Not sent by client
        WebSocketMessageInner::Keepalive { server_time: _ } => {} // Not sent by client
        WebSocketMessageInner::Response {
            responding_to: _,
            data: _,
        } => {} // Not sent by client
        WebSocketMessageInner::Work => {
            let message = WebSocketMessageInner::Response {
                responding_to: "work".to_owned(),
                data: WebSocketMessageResponse::Work { work: 69420 },
            };
            let message =
                serde_json::to_string(&message).expect("Failed to turn response into string");
            let _ = session.text(message).await;
        }
        WebSocketMessageInner::MakeTransaction {
            private_key: _,
            to: _,
            amount: _,
            metadata: _,
        } => todo!(),
        WebSocketMessageInner::GetValidSubscriptionLevels => todo!(),
        WebSocketMessageInner::Address {
            address: _,
            fetch_names: _,
        } => todo!(),
        WebSocketMessageInner::Me => todo!(),
        WebSocketMessageInner::GetSubscriptionLevel => todo!(),
        WebSocketMessageInner::Logout => todo!(),
        WebSocketMessageInner::Login { private_key: _ } => todo!(),
        WebSocketMessageInner::Subscribe { event } => {
            if WebSocketSubscriptionType::is_valid(&event) {
                let event = WebSocketSubscriptionType::from_str(&event).expect("guh");
                let _ = server.subscribe_to_event(uuid, event).await;

                let subscription_list = server.get_subscription_list(uuid).await;
                let subscription_list: Vec<String> = subscription_list
                    .into_iter()
                    .map(|x| x.into_string())
                    .collect();

                let message = WebSocketMessageInner::Response {
                    responding_to: "subscribe".to_owned(),
                    data: WebSocketMessageResponse::Subscribe {
                        subscription_level: subscription_list,
                    },
                };

                let message =
                    serde_json::to_string(&message).expect("Failed to turn response into string");
                let _ = session.text(message).await;
            } else {
                // Send a message to the session
            }
        }
        WebSocketMessageInner::Unsubscribe { event } => {
            if WebSocketSubscriptionType::is_valid(&event) {
                let event = WebSocketSubscriptionType::from_str(&event).expect("guh");
                let _ = server.unsubscribe_from_event(uuid, &event).await;

                let subscription_list = server.get_subscription_list(uuid).await;
                let subscription_list: Vec<String> = subscription_list
                    .into_iter()
                    .map(|x| x.into_string())
                    .collect();

                let message = WebSocketMessageInner::Response {
                    responding_to: "unsubscribe".to_owned(),
                    data: WebSocketMessageResponse::Unsubscribe {
                        subscription_level: subscription_list,
                    },
                };

                let message =
                    serde_json::to_string(&message).expect("Failed to turn response into string");
                let _ = session.text(message).await;
            } else {
                // Send a message to the session
            }
        }
    }
}
