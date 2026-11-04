//! 用默认浏览器打开网页（设计文档第 6.6 节“界面”）。只给 Rust 端用，地址由程序拼出。

use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
use windows::core::{HSTRING, PCWSTR, w};

/// 只打开 https 网址。
pub fn open_in_browser(url: &str) -> Result<(), String> {
    if !url.starts_with("https://") {
        return Err("只能打开 https 网址。".to_owned());
    }
    let result = unsafe {
        ShellExecuteW(
            None,
            w!("open"),
            &HSTRING::from(url),
            PCWSTR::null(),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        )
    };
    // 返回值大于 32 表示成功。
    let code = result.0 as isize;
    if code > 32 {
        Ok(())
    } else {
        Err(format!("打不开浏览器（错误码 {code}）。"))
    }
}
