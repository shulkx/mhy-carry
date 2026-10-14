//! 米哈游接口客户端。只允许访问白名单里的官方域名。
#![forbid(unsafe_code)]

mod bind;
mod client;
mod endpoints;
mod envelope;
pub mod error;
mod guard;
mod login;
mod roles;

pub use bind::{BindQr, BindStatus, BoundAccount};
pub use client::ApiClient;
pub use error::{ApiError, retcode};
pub use login::{Credentials, LoginError, ScannedApp};
