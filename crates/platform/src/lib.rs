//! 和操作系统打交道的部分：数据目录、窗口枚举和截图。截图只有 Windows 实现。

#[cfg(windows)]
pub mod capture;
pub mod paths;
pub mod target;
