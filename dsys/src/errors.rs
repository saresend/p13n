use actix_web::{HttpResponse, ResponseError, http::StatusCode};
use diesel::result::Error as DieselError;
use serde_json::json;
use thiserror::Error;

#[derive(Debug, Error)]
pub(crate) enum ApiError {
    #[error("{0} not found")]
    NotFound(&'static str),
    #[error("{0}")]
    BadRequest(String),
    #[error("{0}")]
    Conflict(String),
    #[error("database error: {0}")]
    Db(#[from] DieselError),
    #[error("connection pool error: {0}")]
    Pool(#[from] diesel::r2d2::PoolError),
    #[error("file storage error: {0}")]
    Io(#[from] std::io::Error),
    #[error("worker thread error: {0}")]
    Blocking(#[from] actix_web::error::BlockingError),
}

impl ResponseError for ApiError {
    fn status_code(&self) -> StatusCode {
        match self {
            ApiError::NotFound(_) => StatusCode::NOT_FOUND,
            ApiError::BadRequest(_) => StatusCode::BAD_REQUEST,
            ApiError::Conflict(_) => StatusCode::CONFLICT,
            ApiError::Db(DieselError::NotFound) => StatusCode::NOT_FOUND,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    fn error_response(&self) -> HttpResponse {
        if self.status_code().is_server_error() {
            log::error!("{self}");
        }
        HttpResponse::build(self.status_code()).json(json!({ "error": self.to_string() }))
    }
}

pub(crate) type ApiResult<T> = Result<T, ApiError>;
