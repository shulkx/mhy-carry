//! MhyCarry 的核心逻辑：二维码校验、游戏配置、账号和设置文件。
//! 这里不访问网络，也不调用操作系统 API。
#![forbid(unsafe_code)]

pub mod games;
pub mod qr;
