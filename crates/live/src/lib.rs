//! 直播间抢码（设计文档第 6.5 节）：直播客户端、各平台取流、直播画面来源。
//! 不依赖 Windows；解码器由调用方传进来。
#![forbid(unsafe_code)]

mod bilibili;
mod client;
mod platforms;
mod source;

pub use client::{LiveClient, LiveError};
pub use source::{DecoderFactory, LiveSource};
