//! 给界面的状态快照（设计文档第 6.3 节）。界面只按它显示，自己不记状态。
//! 快照里永远没有 stoken、mid、cookie_token、游戏登录二维码的内容和截到的画面。唯一的例外是
//! 绑定二维码：第 6.1 节要求把它显示出来，它的 data URI 里含绑定用的 ticket。

use std::collections::HashMap;

use base64::Engine;
use base64::prelude::BASE64_STANDARD;
use mhy_carry_core::account::{Account, AccountFile, GameRole};
use mhy_carry_core::scan::{ScanStatus, StopReason};
use mhy_carry_core::settings::{MIN_IDLE_TIMEOUT_SECS, ScanSettings, TargetKind};
use mhy_carry_core::store::StoreError;
use qrcode::QrCode;
use qrcode::render::svg;
use serde::Serialize;

/// 推送给界面的完整快照。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AppState {
    /// 快照序号，越大越新。推送可能乱序到达，界面丢掉比手上旧的快照。
    pub seq: u64,
    pub accounts: AccountsView,
    pub bind: BindView,
    pub scan: ScanStatus,
    /// 扫描状态的中文说明，和命令行显示的一样。
    pub scan_text: String,
    /// 最近一条提示的中文说明。
    pub notice_text: Option<String>,
    /// 这条提示出现的时间（本地时间，时:分:秒）。
    pub notice_at: Option<String>,
    pub settings: SettingsView,
    /// 账号文件的路径，界面上写明凭证明文保存在哪里（设计文档第 8 节）。
    pub credential_file: String,
    /// 账号文件损坏时留下的备份等旧文件的个数。
    pub leftovers: usize,
}

/// 账号列表（设计文档第 6.4 节）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AccountsView {
    /// 没有账号。`notice` 是要显示的原因，比如账号文件损坏、已经改名备份。
    None { notice: Option<String> },
    /// 有账号。`active` 是使用中的账号的本地 ID。
    List {
        items: Vec<AccountItem>,
        active: String,
    },
    /// 账号文件读不了：格式比本程序新（只读），或者读写出错。绑定、切换、解绑都不可用。
    Unavailable { message: String },
}

/// 列表里的一个账号。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AccountItem {
    /// 本地 ID。切换、写备注、解绑时用它指明账号。
    pub id: String,
    /// 账号名称：备注 → 原神昵称 → 通行证 ID。
    pub name: String,
    pub note: String,
    /// 米哈游通行证 ID。
    pub passport_id: String,
    /// 原神官服角色，还没查到时是 null。
    pub role: Option<GameRole>,
    pub bound_at: String,
    pub credential: Credential,
}

impl AccountItem {
    fn new(account: &Account, credentials: &HashMap<String, Credential>) -> Self {
        Self {
            id: account.id.clone(),
            name: account.display_name(),
            note: account.note.clone(),
            passport_id: account.uid.clone(),
            role: account.game_role.clone(),
            bound_at: account
                .bound_at
                .to_zoned(jiff::tz::TimeZone::system())
                .strftime("%Y-%m-%d")
                .to_string(),
            credential: credentials
                .get(&account.id)
                .copied()
                .unwrap_or(Credential::Unchecked),
        }
    }
}

impl AccountsView {
    /// 从读账号文件的结果得出账号列表。`active` 是设置里记的使用中的账号，
    /// `credentials` 是这次运行里查过的凭证状态。
    pub fn from_load(
        result: Result<AccountFile, StoreError>,
        active: Option<&str>,
        credentials: &HashMap<String, Credential>,
    ) -> Self {
        match result {
            Ok(file) => match file.active(active) {
                Some(current) => AccountsView::List {
                    active: current.id.clone(),
                    items: file
                        .accounts
                        .iter()
                        .map(|account| AccountItem::new(account, credentials))
                        .collect(),
                },
                None => AccountsView::None { notice: None },
            },
            Err(error @ StoreError::Corrupt { .. }) => AccountsView::None {
                notice: Some(error.to_string()),
            },
            Err(error) => AccountsView::Unavailable {
                message: error.to_string(),
            },
        }
    }

    /// 列表里有没有这个账号。
    pub fn contains(&self, id: &str) -> bool {
        matches!(self, AccountsView::List { items, .. } if items.iter().any(|item| item.id == id))
    }

    /// 更新一个账号的凭证状态。
    pub fn set_credential(&mut self, id: &str, credential: Credential) {
        if let AccountsView::List { items, .. } = self
            && let Some(item) = items.iter_mut().find(|item| item.id == id)
        {
            item.credential = credential;
        }
    }
}

/// 账号凭证的状态。只在开始扫描和绑定成功时检查，打开程序时不联网。
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

/// 记下提示出现的时间（设计文档第 6.3 节“最近一条提示，带时间”）。
/// 只有提示的内容变了才更新时间；暂停、等你确认时提示暂时不显示，不算变化。
pub fn track_notice(
    last: &mut Option<String>,
    at: &mut Option<String>,
    status: &ScanStatus,
    now: impl FnOnce() -> String,
) {
    if let Some(notice) = status.notice() {
        let text = notice.to_string();
        if last.as_deref() != Some(text.as_str()) {
            *last = Some(text);
            *at = Some(now());
        }
    }
}

/// 绑定流程（设计文档第 6.1、6.3 节）。绑定成功几秒后自动收起；其他结束状态一直显示，
/// 直到再次绑定或点关闭。
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
    /// `added` 为 false 表示更新了已有账号的凭证。
    Succeeded {
        added: bool,
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
    use mhy_carry_core::account::SecretString;
    use std::path::PathBuf;

    fn account(id: &str, uid: &str) -> Account {
        let mut account = Account::new(
            uid.to_owned(),
            "mid_abc".to_owned(),
            SecretString::from("v2_secret_stoken"),
            "2026-10-12T12:00:00Z".parse().unwrap(),
        );
        account.id = id.to_owned();
        account
    }

    fn sample_file() -> AccountFile {
        let mut second = account("a2", "300067890");
        second.note = "小号".to_owned();
        second.game_role = Some(GameRole {
            nickname: "派蒙".to_owned(),
            level: 35,
            game_uid: "100000002".to_owned(),
        });
        AccountFile {
            accounts: vec![account("a1", "300012345"), second],
            ..AccountFile::empty()
        }
    }

    #[test]
    fn accounts_view_follows_the_file_and_the_active_id() {
        assert_eq!(
            AccountsView::from_load(Ok(AccountFile::empty()), None, &HashMap::new()),
            AccountsView::None { notice: None }
        );

        let credentials = HashMap::from([("a2".to_owned(), Credential::Invalid)]);
        let AccountsView::List { items, active } =
            AccountsView::from_load(Ok(sample_file()), Some("a2"), &credentials)
        else {
            panic!("应该是账号列表");
        };
        assert_eq!(active, "a2");
        assert_eq!(
            items[0],
            AccountItem {
                id: "a1".to_owned(),
                name: "300012345".to_owned(),
                note: String::new(),
                passport_id: "300012345".to_owned(),
                role: None,
                bound_at: "2026-10-12".to_owned(),
                credential: Credential::Unchecked,
            }
        );
        assert_eq!(items[1].name, "小号");
        assert_eq!(items[1].role.as_ref().unwrap().nickname, "派蒙");
        assert_eq!(items[1].credential, Credential::Invalid);
    }

    #[test]
    fn missing_active_account_falls_back_to_the_first() {
        let view = AccountsView::from_load(Ok(sample_file()), Some("gone"), &HashMap::new());
        assert!(
            matches!(&view, AccountsView::List { active, .. } if active == "a1"),
            "{view:?}"
        );
        assert!(view.contains("a2"));
        assert!(!view.contains("gone"));
    }

    #[test]
    fn credential_can_be_updated_in_place() {
        let mut view = AccountsView::from_load(Ok(sample_file()), None, &HashMap::new());
        view.set_credential("a1", Credential::Valid);
        let AccountsView::List { items, .. } = view else {
            panic!("应该是账号列表");
        };
        assert_eq!(items[0].credential, Credential::Valid);
        assert_eq!(items[1].credential, Credential::Unchecked);
    }

    #[test]
    fn corrupt_file_counts_as_unbound_with_a_notice() {
        let view = AccountsView::from_load(
            Err(StoreError::Corrupt {
                backup: PathBuf::from("accounts.json.bak-1"),
            }),
            None,
            &HashMap::new(),
        );
        assert!(
            matches!(&view, AccountsView::None { notice: Some(text) } if text.contains("已损坏")),
            "{view:?}"
        );
    }

    #[test]
    fn newer_file_is_unavailable() {
        let view = AccountsView::from_load(
            Err(StoreError::UnsupportedFormat {
                format: 3,
                encryption: "dpapi".to_owned(),
            }),
            None,
            &HashMap::new(),
        );
        assert!(matches!(view, AccountsView::Unavailable { .. }), "{view:?}");
    }

    #[test]
    fn the_snapshot_never_contains_the_stoken() {
        let view = AccountsView::from_load(Ok(sample_file()), None, &HashMap::new());
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
        assert_eq!(
            serde_json::to_value(BindView::Succeeded { added: true }).unwrap(),
            serde_json::json!({"kind": "succeeded", "added": true})
        );
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

    #[test]
    fn notice_time_changes_only_when_the_notice_changes() {
        use mhy_carry_core::scan::Notice;
        let scanning = |notice| ScanStatus::Scanning {
            source: "YuanShen.exe（原神）".to_owned(),
            notice,
        };
        let (mut last, mut at) = (None, None);
        track_notice(
            &mut last,
            &mut at,
            &scanning(Some(Notice::QrExpired)),
            || "10:00:00".to_owned(),
        );
        assert_eq!(at.as_deref(), Some("10:00:00"));
        // 暂停一下再回到扫描，提示没变，时间也不变。
        track_notice(&mut last, &mut at, &ScanStatus::Paused, || {
            "10:01:00".to_owned()
        });
        track_notice(
            &mut last,
            &mut at,
            &scanning(Some(Notice::QrExpired)),
            || "10:02:00".to_owned(),
        );
        assert_eq!(at.as_deref(), Some("10:00:00"));
        track_notice(
            &mut last,
            &mut at,
            &scanning(Some(Notice::Declined)),
            || "10:03:00".to_owned(),
        );
        assert_eq!(at.as_deref(), Some("10:03:00"));
    }
}
