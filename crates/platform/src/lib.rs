//! 和操作系统打交道的部分：数据目录、窗口枚举、截图和直播画面解码。截图和解码只有 Windows 实现。

#[cfg(windows)]
pub mod capture;
pub mod console;
#[cfg(windows)]
pub mod dialog;
pub mod paths;
pub mod target;
#[cfg(windows)]
pub mod video;
