//! 终端相关。

/// 丢掉终端里已经敲好、还没读走的输入。提问之前调用，免得之前多按的 y 回车
/// 直接回答了新的问题，你却没看到问题。
pub fn discard_typed_ahead() {
    #[cfg(windows)]
    // SAFETY: 只取标准输入的句柄并清空它的输入缓冲。标准输入不是控制台（比如被重定向）
    // 时调用会失败，这时也没有可丢的预先输入，忽略即可。
    unsafe {
        use windows::Win32::System::Console::{
            FlushConsoleInputBuffer, GetStdHandle, STD_INPUT_HANDLE,
        };
        if let Ok(handle) = GetStdHandle(STD_INPUT_HANDLE) {
            let _ = FlushConsoleInputBuffer(handle);
        }
    }
}
