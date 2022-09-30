#![allow(unused_imports)]
use ate::prelude::*;
use serde::*;
use tracing::{debug, error, info, instrument, span, trace, warn, Level};

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct CreateUserRequest {
    pub auth: String,
    pub email: String,
    pub secret: EncryptKey,
    pub accepted_terms: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct CreateUserResponse {
    pub key: PrimaryKey,
    pub qr_code: String,
    pub qr_secret: String,
    pub recovery_code: String,
    pub authority: AteSessionUser,
    pub message_of_the_day: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub enum CreateUserFailed {
    AlreadyExists(String),
    InvalidEmail,
    NoMoreRoom,
    NoMasterKey,
    TermsAndConditions(String),
    InternalError(u16),
}

impl<E> From<E> for CreateUserFailed
where
    E: std::error::Error + Sized,
{
    fn from(err: E) -> Self {
        CreateUserFailed::InternalError(ate::utils::obscure_error(err))
    }
}
