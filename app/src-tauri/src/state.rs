//! 给界面的状态快照（设计文档第 6.3 节）。界面只按它显示，自己不记状态。
//! 快照里永远没有 stoken、mid、ticket、二维码内容和截到的画面。

use base64::Engine;
use base64::prelude::BASE64_STANDARD;
use mhy_carry_core::account::AccountFile;
use mhy_carry_core::scan::{ScanStatus, StopReason};
use mhy_carry_core::settings::{MIN_IDLE_TIMEOUT_SECS, ScanSettings, TargetKind};
use mhy_carry_core::store::StoreError;
use qrcode::QrCode;
use qrcode::render::svg;
use serde::Serialize;

/// 推送给界面的完整快照。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AppState {
    pub account: AccountView,
    pub bind: BindView,
    pub scan: ScanStatus,
    /// 扫描状态的中文说明，和命令行显示的一样。
    pub scan_text: String,
    /// 最近一条提示的中文说明。
    pub notice_text: Option<String>,
    pub settings: SettingsView,
    /// 账号文件的路径，界面上写明凭证明文保存在哪里（设计文档第 8 节）。
    pub credential_file: String,
    /// 账号文件损坏时留下的备份等旧文件的个数。
    pub leftovers: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AccountView {
    /// 未绑定。`notice` 是要显示的原因，比如账号文件损坏、已经改名备份。
    None { notice: Option<String> },
    /// 已绑定。uid 完整显示。
    Bound {
        uid: String,
        bound_at: String,
        credential: Credential,
    },
    /// 账号文件读不了：格式比本程序新（只读），或者读写出错。绑定和解绑都不可用。
    Unavailable { message: String },
}

impl AccountView {
    /// 从读账号文件的结果得出账号状态。
    pub fn from_load(result: Result<AccountFile, StoreError>) -> Self {
        match result {
            Ok(file) => match file.accounts.into_iter().next() {
                Some(account) => AccountView::Bound {
                    uid: account.uid,
                    bound_at: account
                        .bound_at
                        .to_zoned(jiff::tz::TimeZone::system())
                        .strftime("%Y-%m-%d")
                        .to_string(),
                    credential: Credential::Unchecked,
                },
                None => AccountView::None { notice: None },
            },
            Err(error @ StoreError::Corrupt { .. }) => AccountView::None {
                notice: Some(error.to_string()),
            },
            Err(error) => AccountView::Unavailable {
                message: error.to_string(),
            },
        }
    }
}

/// 账号凭证的状态。只在开始扫描时检查，打开程序时不联网。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Credential {
    Unchecked,
    Valid,
    Invalid,
}

/// 扫描状态变化后，凭证状态跟着变：过了检查凭证这一步就是有效；结束原因是凭证失效就是已失效。
pub fn credential_after(status: &ScanStatus) -> Option<Credential> {
    match status {
        ScanStatus::Idle {
            last: Some(StopReason::CredentialInvalid),
        } => Some(Credential::Invalid),
        ScanStatus::Idle { .. } | ScanStatus::CheckingCredential => None,
        _ => Some(Credential::Valid),
    }
}

/// 绑定流程（设计文档第 6.1、6.3 节）。结束状态一直显示，直到再次绑定或点关闭。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BindView {
    Idle,
    Requesting,
    /// `qr` 是绑定二维码的 SVG data URI。
    WaitingScan {
        qr: String,
    },
    WaitingConfirm {
        qr: String,
    },
    Succeeded {
        uid: String,
    },
    Expired,
    Cancelled,
    Failed {
        message: String,
    },
}

impl BindView {
    /// 绑定是否还在进行。
    pub fn in_progress(&self) -> bool {
        matches!(
            self,
            BindView::Requesting | BindView::WaitingScan { .. } | BindView::WaitingConfirm { .. }
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SettingsView {
    pub default_target: TargetKind,
    pub idle_timeout_secs: u64,
}

/// 网页端提交的设置，超时不短于 30 秒。
pub fn validated_settings(default_target: TargetKind, idle_timeout_secs: u64) -> ScanSettings {
    ScanSettings {
        default_target,
        idle_timeout_secs: idle_timeout_secs.max(MIN_IDLE_TIMEOUT_SECS),
    }
}

/// 把绑定二维码画成 SVG，编码成 data URI 给界面的 `<img>` 用（CSP 只放行 data: 图片）。
pub fn qr_data_uri(content: &str) -> Result<String, qrcode::types::QrError> {
    let svg = QrCode::new(content.as_bytes())?
        .render::<svg::Color<'_>>()
        .min_dimensions(200, 200)
        .dark_color(svg::Color("#000000"))
        .light_color(svg::Color("#ffffff"))
        .build();
    Ok(format!(
        "data:image/svg+xml;base64,{}",
        BASE64_STANDARD.encode(svg)
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use mhy_carry_core::account::{Account, SecretString};
    use std::path::PathBuf;

    fn file_with_account() -> AccountFile {
        AccountFile {
            accounts: vec![Account::new(
                "300012345".to_owned(),
                "mid_abc".to_owned(),
                SecretString::from("v2_secret_stoken"),
                "2026-10-12T12:00:00Z".parse().unwrap(),
            )],
            ..AccountFile::empty()
        }
    }

    #[test]
    fn account_view_follows_the_account_file() {
        assert_eq!(
            AccountView::from_load(Ok(AccountFile::empty())),
            AccountView::None { notice: None }
        );
        assert_eq!(
            AccountView::from_load(Ok(file_with_account())),
            AccountView::Bound {
                uid: "300012345".to_owned(),
                bound_at: "2026-10-12".to_owned(),
                credential: Credential::Unchecked,
            }
        );
    }

    #[test]
    fn corrupt_file_counts_as_unbound_with_a_notice() {
        let view = AccountView::from_load(Err(StoreError::Corrupt {
            backup: PathBuf::from("accounts.json.bak-1"),
        }));
        assert!(
            matches!(&view, AccountView::None { notice: Some(text) } if text.contains("已损坏")),
            "{view:?}"
        );
    }

    #[test]
    fn newer_file_is_unavailable() {
        let view = AccountView::from_load(Err(StoreError::UnsupportedFormat {
            format: 2,
            encryption: "dpapi".to_owned(),
        }));
        assert!(matches!(view, AccountView::Unavailable { .. }), "{view:?}");
    }

    #[test]
    fn the_snapshot_never_contains_the_stoken() {
        let view = AccountView::from_load(Ok(file_with_account()));
        let json = serde_json::to_string(&view).unwrap();
        assert!(!json.contains("v2_secret_stoken"), "{json}");
        assert!(!json.contains("mid_abc"), "{json}");
    }

    #[test]
    fn credential_follows_the_scan() {
        assert_eq!(credential_after(&ScanStatus::CheckingCredential), None);
        assert_eq!(
            credential_after(&ScanStatus::WaitingForWindow {
                game: "原神".to_owned()
            }),
            Some(Credential::Valid)
        );
        assert_eq!(
            credential_after(&ScanStatus::Idle {
                last: Some(StopReason::CredentialInvalid)
            }),
            Some(Credential::Invalid)
        );
        assert_eq!(
            credential_after(&ScanStatus::Idle {
                last: Some(StopReason::UserStopped)
            }),
            None
        );
    }

    #[test]
    fn bind_view_is_a_tagged_snapshot() {
        let view = BindView::WaitingScan {
            qr: "data:image/svg+xml;base64,AAAA".to_owned(),
        };
        assert_eq!(
            serde_json::to_value(&view).unwrap(),
            serde_json::json!({"kind": "waiting_scan", "qr": "data:image/svg+xml;base64,AAAA"})
        );
        assert!(view.in_progress());
        assert!(!BindView::Expired.in_progress());
    }

    #[test]
    fn bind_qr_is_an_svg_data_uri() {
        let uri = qr_data_uri("https://user.mihoyo.com/login-platform/x").unwrap();
        let encoded = uri.strip_prefix("data:image/svg+xml;base64,").unwrap();
        let svg = String::from_utf8(BASE64_STANDARD.decode(encoded).unwrap()).unwrap();
        assert!(svg.contains("<svg"), "{svg}");
    }

    #[test]
    fn settings_timeout_has_a_floor() {
        assert_eq!(
            validated_settings(TargetKind::Monitor, 5).idle_timeout_secs,
            30
        );
        assert_eq!(
            validated_settings(TargetKind::Monitor, 600).idle_timeout_secs,
            600
        );
    }
}
