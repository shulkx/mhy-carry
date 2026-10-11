//! 米哈游接口客户端。只允许访问白名单里的官方域名。
#![forbid(unsafe_code)]
// 临时：任务 15 完成后删除。
#![allow(dead_code)]

mod client;
mod endpoints;
mod envelope;
pub mod error;
mod guard;
mod login;

pub use client::ApiClient;
pub use error::{ApiError, retcode};
pub use login::{Credentials, ScannedApp};
