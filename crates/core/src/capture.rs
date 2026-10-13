//! 画面来源（设计文档第 5 节）。具体的截图实现在 platform 里。

use serde::Serialize;

use crate::text::printable;

/// 一帧灰度画面，每个像素一个字节，逐行排列。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub luma: Vec<u8>,
}

/// 截图目标的种类。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceKind {
    /// 自动找到的游戏窗口。
    GameWindow,
    /// 你从列表里选的窗口，比如正在看直播的浏览器。
    PickedWindow,
    /// 整个显示器。
    Monitor,
}

/// 画面来自哪里，用来告诉你二维码是在哪发现的。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SourceInfo {
    pub kind: SourceKind,
    /// 给人看的描述，比如 `YuanShen.exe（原神）`。
    pub label: String,
}

impl SourceInfo {
    /// `label` 常带着窗口标题这类外部文本，先清理掉控制字符。
    pub fn new(kind: SourceKind, label: &str) -> Self {
        Self {
            kind,
            label: printable(label),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CaptureError {
    /// 自动模式下还没找到游戏窗口。
    #[error("还没找到游戏窗口")]
    NotFound,
    #[error("窗口已最小化")]
    Minimized,
    /// 自动模式下游戏窗口不在最前面。截的是窗口所在的屏幕区域，这时可能截到盖在上面的窗口。
    #[error("游戏窗口不在最前面")]
    InBackground,
    /// 指定的窗口关了，或者指定的显示器断开了。
    #[error("截图目标已经不在了")]
    Gone,
    #[error("截图失败：{0}")]
    Failed(String),
}

/// 一帧帧地提供画面。
pub trait FrameSource {
    fn next_frame(&mut self) -> Result<Frame, CaptureError>;
    /// 最近一次截到的画面来自哪里。
    fn describe(&self) -> SourceInfo;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_label_is_cleaned() {
        let info = SourceInfo::new(
            SourceKind::PickedWindow,
            "直播间\r在 YuanShen.exe（原神）中发现",
        );
        assert_eq!(info.label, "直播间 在 YuanShen.exe（原神）中发现");
    }
}
