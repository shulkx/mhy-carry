//! Windows 系统确认框（设计文档第 6.2 节第 5 步、第 9 节）。网页端没有对话框权限，
//! 确认框只能从 Rust 端弹出。

use std::ffi::c_void;

use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::{
    FLASHW_ALL, FLASHW_TIMERNOFG, FLASHWINFO, FlashWindowEx, IDYES, MB_DEFBUTTON2, MB_ICONWARNING,
    MB_SETFOREGROUND, MB_TOPMOST, MB_YESNO, MESSAGEBOX_STYLE, MessageBoxW,
};
use windows::core::HSTRING;

const TITLE: &str = "MhyCarry";

/// 弹出“是 / 否”确认框，阻塞到你回答为止，返回你是否点了“是”。
///
/// 默认按钮是“否”：确认框突然弹出时误按回车或空格只会取消。带警告图标，弹出时会响
/// 系统提示音。`topmost` 为真时置顶并尝试切到前台，用于登录确认：发现二维码时游戏
/// 在最前面，不置顶就会被挡住。`owner` 是所属窗口的句柄，确认框显示在它前面。
pub fn confirm(owner: Option<isize>, text: &str, topmost: bool) -> bool {
    let owner = owner.map(|raw| HWND(raw as *mut c_void));
    // SAFETY: 两个字符串在调用期间一直有效，MessageBoxW 只读取它们。
    let answer = unsafe {
        MessageBoxW(
            owner,
            &HSTRING::from(text),
            &HSTRING::from(TITLE),
            style(topmost),
        )
    };
    answer == IDYES
}

/// 闪烁窗口在任务栏上的按钮，直到窗口回到前台。
pub fn flash(window: isize) {
    let info = FLASHWINFO {
        cbSize: size_of::<FLASHWINFO>() as u32,
        hwnd: HWND(window as *mut c_void),
        dwFlags: FLASHW_ALL | FLASHW_TIMERNOFG,
        uCount: 0,
        dwTimeout: 0,
    };
    // SAFETY: info 在调用期间一直有效，系统只读取它；句柄无效时调用只是失败。
    unsafe {
        let _ = FlashWindowEx(&info);
    }
}

fn style(topmost: bool) -> MESSAGEBOX_STYLE {
    let style = MB_YESNO | MB_ICONWARNING | MB_DEFBUTTON2;
    if topmost {
        style | MB_TOPMOST | MB_SETFOREGROUND
    } else {
        style
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn has(style: MESSAGEBOX_STYLE, flag: MESSAGEBOX_STYLE) -> bool {
        style.0 & flag.0 == flag.0
    }

    #[test]
    fn default_button_is_no() {
        for topmost in [false, true] {
            assert!(has(style(topmost), MB_YESNO));
            assert!(has(style(topmost), MB_DEFBUTTON2));
        }
    }

    #[test]
    fn only_the_login_confirmation_is_topmost() {
        assert!(has(style(true), MB_TOPMOST));
        assert!(has(style(true), MB_SETFOREGROUND));
        assert!(!has(style(false), MB_TOPMOST));
    }
}
