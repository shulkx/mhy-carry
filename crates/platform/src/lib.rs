//! 和操作系统打交道的部分：数据目录、窗口枚举、截图、直播画面解码，以及自动更新用到的
//! 注册表和浏览器。截图、解码、注册表和浏览器只有 Windows 实现。

#[cfg(windows)]
pub mod capture;
pub mod console;
#[cfg(windows)]
pub mod dialog;
#[cfg(windows)]
pub mod install;
pub mod paths;
#[cfg(windows)]
pub mod shell;
pub mod target;
#[cfg(windows)]
pub mod video;
