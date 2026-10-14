//! 确认策略（设计文档第 5 节）：识别到合法的登录二维码后怎么做。
//!
//! v0.3 起自动确认由你决定开不开，四种画面来源都可以开，不再有按来源区分的硬性规则
//! （设计文档第 6.5、9 节）。同一帧有多个登录码时不登录、确认前核对应用 ID 这两条，
//! 写在扫码会话和登录流程里，任何策略都绕不开。

use crate::capture::SourceInfo;
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

/// 每次都问你。“自动确认”关着时用它。
#[derive(Debug, Default, Clone, Copy)]
pub struct AlwaysAsk;

impl ConfirmPolicy for AlwaysAsk {
    fn decide(&mut self, _source: &SourceInfo, _qr: &LoginQr) -> Decision {
        Decision::AskUser
    }
}

/// 识别到合法的登录码就直接登录。“自动确认”开着时用它（设计文档第 6.5 节）。
#[derive(Debug, Default, Clone, Copy)]
pub struct AlwaysAutoConfirm;

impl ConfirmPolicy for AlwaysAutoConfirm {
    fn decide(&mut self, _source: &SourceInfo, _qr: &LoginQr) -> Decision {
        Decision::AutoConfirm
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capture::SourceKind;
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
    fn auto_confirm_confirms_every_source() {
        for kind in [
            SourceKind::GameWindow,
            SourceKind::PickedWindow,
            SourceKind::Monitor,
            SourceKind::LiveStream,
        ] {
            let source = SourceInfo::new(kind, "x");
            assert_eq!(
                AlwaysAutoConfirm.decide(&source, &qr()),
                Decision::AutoConfirm,
                "{kind:?}"
            );
        }
    }
}
