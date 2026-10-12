//! 确认策略（设计文档第 5 节）：识别到合法的登录二维码后怎么做。

use crate::capture::{SourceInfo, SourceKind};
use crate::qr::LoginQr;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// 问你。
    AskUser,
    /// 直接登录。
    AutoConfirm,
    /// 忽略这个二维码。
    Ignore,
}

pub trait ConfirmPolicy {
    fn decide(&mut self, source: &SourceInfo, qr: &LoginQr) -> Decision;
}

/// v1 唯一的策略：每次都问你。
#[derive(Debug, Default, Clone, Copy)]
pub struct AlwaysAsk;

impl ConfirmPolicy for AlwaysAsk {
    fn decide(&mut self, _source: &SourceInfo, _qr: &LoginQr) -> Decision {
        Decision::AskUser
    }
}

/// 硬性规则：画面来自指定窗口或显示器时，永远不自动确认。那里的画面可能是
/// 别人的直播，二维码可能是别人故意放的。任何策略的决定都要先过这一关。
pub fn enforce_hard_rules(decision: Decision, source: &SourceInfo) -> Decision {
    match (decision, source.kind) {
        (Decision::AutoConfirm, SourceKind::PickedWindow | SourceKind::Monitor) => {
            Decision::AskUser
        }
        (decision, _) => decision,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::qr::parse_login_qr;

    fn qr() -> LoginQr {
        parse_login_qr(
            "https://user.mihoyo.com/qr_code_in_game.html?app_id=4&biz_key=hk4e_cn&expire=4102444800&ticket=66cf3afb2a1c7a308832bd56",
            1_760_000_000,
        )
        .unwrap()
    }

    #[test]
    fn always_ask_asks() {
        let source = SourceInfo::new(SourceKind::GameWindow, "YuanShen.exe（原神）");
        assert_eq!(AlwaysAsk.decide(&source, &qr()), Decision::AskUser);
    }

    #[test]
    fn auto_confirm_is_only_allowed_for_the_game_window() {
        let cases = [
            (SourceKind::GameWindow, Decision::AutoConfirm),
            (SourceKind::PickedWindow, Decision::AskUser),
            (SourceKind::Monitor, Decision::AskUser),
        ];
        for (kind, expected) in cases {
            let source = SourceInfo::new(kind, "x");
            assert_eq!(
                enforce_hard_rules(Decision::AutoConfirm, &source),
                expected,
                "{kind:?}"
            );
            assert_eq!(
                enforce_hard_rules(Decision::Ignore, &source),
                Decision::Ignore
            );
        }
    }
}
