//! MhyCarry 的核心逻辑：二维码校验、游戏配置、账号和设置文件。
//! 这里不访问网络，也不调用操作系统 API。
#![forbid(unsafe_code)]

pub mod account;
pub mod fsutil;
pub mod games;
pub mod qr;
pub mod settings;
pub mod store;
