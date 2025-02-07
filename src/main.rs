use actix_web::{App, HttpResponse, HttpServer, get, middleware::Logger, web};

use actix_ws_fuckery::ws::{self, WebSocketServer};
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
struct MyObj {
    number: i32,
}

#[get("/")]
async fn index(
    item: web::Json<MyObj>,
    server: web::Data<WebSocketServer>,
) -> Result<HttpResponse, actix_web::Error> {
    let item = item.into_inner();
    let string = serde_json::to_string(&item).expect("fucked up");

    server.broadcast(string).await;

    Ok(HttpResponse::Ok().body("Sent number to clients :3"))
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let websocket_server = WebSocketServer::new();

    HttpServer::new(move || {
        App::new()
            .wrap(Logger::default())
            .app_data(web::Data::new(websocket_server.clone()))
            .service(ws::ws_handler)
            .service(ws::start_ws)
            .service(index)
    })
    .bind("127.0.0.1:8080")?
    .run()
    .await?;

    Ok(())
}
