//! HTTP API:
//!   POST /init                        a JSON Schema for `{ action_name, payload }`
//!   POST /action                      one action (see `Action`)
//!   GET  /sse/suggestion?user_id=...  server-sent `suggestion` events for that user
//!   GET  /health

use crate::action::Action;
use crate::engine::{Engine, EngineError};
use actix_web::http::StatusCode;
use actix_web::{HttpResponse, ResponseError, get, post, web};
use bytes::Bytes;
use serde::Deserialize;
use serde_json::{Value, json};
use std::fmt;
use std::time::Duration;

const KEEPALIVE: Duration = Duration::from_secs(15);

pub fn configure(cfg: &mut web::ServiceConfig) {
    cfg.app_data(web::JsonConfig::default().limit(1 << 20))
        .service(health)
        .service(init)
        .service(action)
        .service(suggestions);
}

impl fmt::Display for EngineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EngineError::NotInitialized => {
                write!(f, "no vocabulary yet; POST a JSON Schema to /init first")
            }
            EngineError::InvalidSchema(error) => write!(f, "invalid JSON Schema: {error}"),
            EngineError::InvalidAction(_) => write!(f, "action does not match the vocabulary"),
            EngineError::Db(_) => write!(f, "database error"),
        }
    }
}

impl ResponseError for EngineError {
    fn status_code(&self) -> StatusCode {
        match self {
            EngineError::NotInitialized => StatusCode::CONFLICT,
            EngineError::InvalidSchema(_) => StatusCode::BAD_REQUEST,
            EngineError::InvalidAction(_) => StatusCode::UNPROCESSABLE_ENTITY,
            EngineError::Db(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    fn error_response(&self) -> HttpResponse {
        if let EngineError::Db(error) = self {
            log::error!("{error}");
        }
        let mut body = json!({ "error": self.to_string() });
        if let EngineError::InvalidAction(details) = self {
            body["details"] = json!(details);
        }
        HttpResponse::build(self.status_code()).json(body)
    }
}

fn blocking_error(error: actix_web::error::BlockingError) -> actix_web::Error {
    actix_web::error::ErrorInternalServerError(error)
}

#[get("/health")]
async fn health() -> HttpResponse {
    HttpResponse::Ok().json(json!({ "ok": true }))
}

#[post("/init")]
async fn init(
    engine: web::Data<Engine>,
    schema: web::Json<Value>,
) -> actix_web::Result<HttpResponse> {
    let engine = engine.into_inner();
    web::block(move || engine.init(schema.into_inner()))
        .await
        .map_err(blocking_error)??;
    Ok(HttpResponse::Ok().json(json!({ "ok": true })))
}

#[post("/action")]
async fn action(
    engine: web::Data<Engine>,
    action: web::Json<Action>,
) -> actix_web::Result<HttpResponse> {
    let engine = engine.into_inner();
    let id = web::block(move || engine.record(&action))
        .await
        .map_err(blocking_error)??;
    Ok(HttpResponse::Accepted().json(json!({ "id": id })))
}

#[derive(Deserialize)]
struct StreamQuery {
    user_id: String,
}

#[get("/sse/suggestion")]
async fn suggestions(engine: web::Data<Engine>, query: web::Query<StreamQuery>) -> HttpResponse {
    let subscription = engine.queue(&query.user_id).subscribe();
    let opened = futures_util::stream::once(async {
        Ok::<_, actix_web::Error>(Bytes::from_static(b": connected\n\n"))
    });
    let events = futures_util::stream::unfold(subscription, |mut sub| async move {
        let chunk = tokio::select! {
            suggestion = sub.next() => {
                let data = serde_json::to_string(&suggestion?).unwrap_or_else(|_| "null".into());
                format!("event: suggestion\ndata: {data}\n\n")
            }
            // Comments keep proxies from closing the stream and reveal dead clients.
            _ = tokio::time::sleep(KEEPALIVE) => ": keepalive\n\n".to_owned(),
        };
        Some((Ok::<_, actix_web::Error>(Bytes::from(chunk)), sub))
    });
    HttpResponse::Ok()
        .content_type("text/event-stream")
        .insert_header(("Cache-Control", "no-cache"))
        .insert_header(("X-Accel-Buffering", "no"))
        .streaming(futures_util::StreamExt::chain(opened, events))
}
