//! Windows 系统确认框（设计文档第 6.2 节第 5 步、第 9 节）。网页端没有对话框权限，
//! 确认框只能从 Rust 端弹出。

use std::ffi::c_void;

use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::{
    FLASHW_ALL, FLASHW_TIMERNOFG, FLASHWINFO, FlashWindowEx, IDOK, MB_DEFBUTTON2, MB_ICONERROR,
    MB_ICONWARNING, MB_OK, MB_OKCANCEL, MB_SETFOREGROUND, MB_TOPMOST, MESSAGEBOX_STYLE,
    MessageBoxW,
};
use windows::core::HSTRING;

const TITLE: &str = "MhyCarry";

/// 弹出“确定 / 取消”确认框，阻塞到你回答为止，返回你是否点了“确定”。
///
/// 默认按钮是“取消”，两个按钮都没有快捷字母：确认框突然弹出时，误按回车、空格、Esc
/// 或任何字母键都不会确认，只能用鼠标点“确定”（换成“是 / 否”的话，单按 Y 就会确认）。
/// 带警告图标，弹出时会响系统提示音。`topmost` 为真时置顶并尝试切到前台，用于登录确认：
/// 发现二维码时游戏在最前面，不置顶就会被挡住。`owner` 是所属窗口的句柄，确认框显示在它前面。
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
    answer == IDOK
}

/// 弹出只有“确定”的错误提示，比如程序启动失败时：正式版没有控制台，不弹窗就什么都看不到。
pub fn alert(text: &str) {
    // SAFETY: 两个字符串在调用期间一直有效，MessageBoxW 只读取它们。
    unsafe {
        MessageBoxW(
            None,
            &HSTRING::from(text),
            &HSTRING::from(TITLE),
            MB_OK | MB_ICONERROR,
        );
    }
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
    let style = MB_OKCANCEL | MB_ICONWARNING | MB_DEFBUTTON2;
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

    /// 低 4 位是按钮组合。
    const BUTTONS_MASK: u32 = 0xF;

    #[test]
    fn buttons_are_ok_and_cancel_with_cancel_as_default() {
        // “是(Y) / 否(N)”带快捷字母，单按 Y 就会确认；“确定 / 取消”没有快捷字母。
        for topmost in [false, true] {
            assert_eq!(style(topmost).0 & BUTTONS_MASK, MB_OKCANCEL.0);
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
