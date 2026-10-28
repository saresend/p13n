use std::fmt::Display;

use actix_web::error;
use diesel::result::Error as DieselError;
use thiserror::Error;

use crate::errors::PError::DBError;

#[derive(Debug, Error)]
pub(crate) enum PError {
    DBError(DieselError),
}

impl Display for PError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DBError(derror) => derror.fmt(f),
        }
    }
}

impl error::ResponseError for PError {}
