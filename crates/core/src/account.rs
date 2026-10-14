//! 账号文件 `accounts.json` 的数据结构和多账号的规则（设计文档第 6.4、8 节）。明文保存。

pub use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize, Serializer};

use crate::text::printable;

/// 本程序写出的账号文件格式版本。格式 2（v0.2）比格式 1 多了 `game_role`；
/// 读到格式 1 的文件照常使用，下次写入时升级。
pub const ACCOUNT_FILE_FORMAT: u32 = 2;
/// 不加密。
pub const ENCRYPTION_NONE: &str = "none";
/// 备注最多这么多个字。
pub const MAX_NOTE_CHARS: usize = 20;

#[derive(Debug, Serialize, Deserialize)]
pub struct AccountFile {
    pub format: u32,
    pub encryption: String,
    /// 按绑定的先后排列。
    pub accounts: Vec<Account>,
}

impl AccountFile {
    pub fn empty() -> Self {
        Self {
            format: ACCOUNT_FILE_FORMAT,
            encryption: ENCRYPTION_NONE.to_owned(),
            accounts: Vec::new(),
        }
    }

    pub fn get(&self, id: &str) -> Option<&Account> {
        self.accounts.iter().find(|account| account.id == id)
    }

    pub fn get_mut(&mut self, id: &str) -> Option<&mut Account> {
        self.accounts.iter_mut().find(|account| account.id == id)
    }

    /// 使用中的账号：`active` 指向的账号；没设过或者它已经不在了，就用第一个。
    pub fn active(&self, active: Option<&str>) -> Option<&Account> {
        active
            .and_then(|id| self.get(id))
            .or_else(|| self.accounts.first())
    }

    /// 保存绑定成功的账号（设计文档第 6.1 节）。通行证 ID 已经在列表里，就只更新凭证和
    /// 绑定时间，备注和原神角色不变；否则加到末尾。
    pub fn upsert(&mut self, account: Account) -> Upserted {
        if let Some(existing) = self.accounts.iter_mut().find(|a| a.uid == account.uid) {
            existing.mid = account.mid;
            existing.stoken = account.stoken;
            existing.bound_at = account.bound_at;
            return Upserted {
                id: existing.id.clone(),
                added: false,
            };
        }
        let id = account.id.clone();
        self.accounts.push(account);
        Upserted { id, added: true }
    }

    /// 解绑一个账号，返回删掉的账号；没有这个账号时返回 None。
    pub fn remove(&mut self, id: &str) -> Option<Account> {
        let index = self.accounts.iter().position(|account| account.id == id)?;
        Some(self.accounts.remove(index))
    }
}

/// [`AccountFile::upsert`] 的结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Upserted {
    /// 账号的本地 ID。
    pub id: String,
    /// 是新加的账号，而不是更新了已有账号的凭证。
    pub added: bool,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Account {
    /// 本地生成的 UUID，只用来区分账号。
    pub id: String,
    /// 米哈游通行证 ID，不是原神里的 UID。
    pub uid: String,
    pub mid: String,
    #[serde(serialize_with = "serialize_secret")]
    pub stoken: SecretString,
    /// 自己写的备注，可以为空（设计文档第 6.4 节）。
    #[serde(default)]
    pub note: String,
    pub bound_at: jiff::Timestamp,
    /// 查到的原神官服角色。格式 1 的文件里没有这一项；还没查到时是 None。
    #[serde(default)]
    pub game_role: Option<GameRole>,
}

/// 原神官服的角色（设计文档第 6.4 节）。不是凭证。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GameRole {
    pub nickname: String,
    pub level: u32,
    /// 原神里的 UID。
    pub game_uid: String,
}

impl Account {
    pub fn new(uid: String, mid: String, stoken: SecretString, bound_at: jiff::Timestamp) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            uid,
            mid,
            stoken,
            note: String::new(),
            bound_at,
            game_role: None,
        }
    }

    /// 账号名称：备注 → 原神昵称 → 通行证 ID，取第一个有值的。
    pub fn display_name(&self) -> String {
        if !self.note.is_empty() {
            return self.note.clone();
        }
        match &self.game_role {
            Some(role) if !role.nickname.is_empty() => role.nickname.clone(),
            _ => self.uid.clone(),
        }
    }

    /// 确认框里怎么称呼这个账号（设计文档第 6.2 节第 5 步），比如
    /// `账号「大号」（原神 UID 100000001，通行证 300012345）`。
    /// 既没有备注也没查到原神角色时，名称就是通行证 ID，写成 `通行证 300012345 的账号`。
    pub fn describe(&self) -> String {
        match (&self.game_role, self.note.is_empty()) {
            (None, true) => format!("通行证 {} 的账号", self.uid),
            (None, false) => format!("账号「{}」（通行证 {}）", self.note, self.uid),
            (Some(role), _) => format!(
                "账号「{}」（原神 UID {}，通行证 {}）",
                self.display_name(),
                role.game_uid,
                self.uid
            ),
        }
    }
}

/// 清理备注：控制字符和看不见的字符换成空格、去掉首尾空白，最多 [`MAX_NOTE_CHARS`] 个字。
pub fn normalize_note(input: &str) -> String {
    let cleaned: String = printable(input).chars().take(MAX_NOTE_CHARS).collect();
    cleaned.trim_end().to_owned()
}

fn serialize_secret<S: Serializer>(
    secret: &SecretString,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(secret.expose_secret())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn account(uid: &str) -> Account {
        Account::new(
            uid.to_owned(),
            "mid_abc".to_owned(),
            SecretString::from("v2_secret_stoken"),
            "2026-10-11T08:00:00Z".parse().unwrap(),
        )
    }

    fn role(nickname: &str) -> GameRole {
        GameRole {
            nickname: nickname.to_owned(),
            level: 60,
            game_uid: "100000001".to_owned(),
        }
    }

    fn file_with(accounts: Vec<Account>) -> AccountFile {
        AccountFile {
            accounts,
            ..AccountFile::empty()
        }
    }

    #[test]
    fn debug_output_never_contains_the_stoken() {
        let debug = format!("{:?}", file_with(vec![account("300012345")]));
        assert!(!debug.contains("v2_secret_stoken"), "{debug}");
    }

    #[test]
    fn round_trips_through_json() {
        let mut first = account("300012345");
        first.note = "大号".to_owned();
        first.game_role = Some(role("旅行者"));
        let json = serde_json::to_string(&file_with(vec![first])).unwrap();
        assert!(json.contains(r#""format":2"#), "{json}");
        assert!(json.contains(r#""encryption":"none""#), "{json}");
        assert!(json.contains(r#""stoken":"v2_secret_stoken""#), "{json}");
        assert!(
            json.contains(r#""bound_at":"2026-10-11T08:00:00Z""#),
            "{json}"
        );

        let back: AccountFile = serde_json::from_str(&json).unwrap();
        let account = &back.accounts[0];
        assert_eq!(account.uid, "300012345");
        assert_eq!(account.mid, "mid_abc");
        assert_eq!(account.stoken.expose_secret(), "v2_secret_stoken");
        assert_eq!(account.note, "大号");
        assert_eq!(account.game_role, Some(role("旅行者")));
    }

    #[test]
    fn format_1_accounts_have_no_game_role() {
        let json = r#"{"format":1,"encryption":"none","accounts":[{"id":"a1","uid":"300012345","mid":"mid_abc","stoken":"v2_secret_stoken","note":"","bound_at":"2026-10-11T08:00:00Z"}]}"#;
        let file: AccountFile = serde_json::from_str(json).unwrap();
        assert_eq!(file.accounts[0].game_role, None);
    }

    #[test]
    fn new_accounts_get_distinct_ids() {
        assert_ne!(account("300012345").id, account("300012345").id);
    }

    #[test]
    fn display_name_prefers_note_then_nickname_then_passport_id() {
        let mut account = account("300012345");
        assert_eq!(account.display_name(), "300012345");
        account.game_role = Some(role("旅行者"));
        assert_eq!(account.display_name(), "旅行者");
        account.note = "大号".to_owned();
        assert_eq!(account.display_name(), "大号");
    }

    #[test]
    fn describe_names_the_account_and_both_ids() {
        let mut account = account("300012345");
        assert_eq!(account.describe(), "通行证 300012345 的账号");
        account.note = "大号".to_owned();
        assert_eq!(account.describe(), "账号「大号」（通行证 300012345）");
        account.game_role = Some(role("旅行者"));
        assert_eq!(
            account.describe(),
            "账号「大号」（原神 UID 100000001，通行证 300012345）"
        );
        account.note.clear();
        assert_eq!(
            account.describe(),
            "账号「旅行者」（原神 UID 100000001，通行证 300012345）"
        );
    }

    #[test]
    fn upsert_adds_new_accounts_at_the_end() {
        let mut file = file_with(vec![account("300012345")]);
        let other = account("300067890");
        let id = other.id.clone();
        assert_eq!(file.upsert(other), Upserted { id, added: true });
        assert_eq!(file.accounts.len(), 2);
        assert_eq!(file.accounts[1].uid, "300067890");
    }

    #[test]
    fn upsert_updates_the_credential_of_a_known_account() {
        let mut existing = account("300012345");
        existing.note = "大号".to_owned();
        existing.game_role = Some(role("旅行者"));
        let id = existing.id.clone();
        let mut file = file_with(vec![existing]);

        let again = Account::new(
            "300012345".to_owned(),
            "mid_new".to_owned(),
            SecretString::from("v2_new_stoken"),
            "2026-10-14T08:00:00Z".parse().unwrap(),
        );
        assert_eq!(
            file.upsert(again),
            Upserted {
                id: id.clone(),
                added: false
            }
        );
        assert_eq!(file.accounts.len(), 1);
        let account = file.get(&id).unwrap();
        assert_eq!(account.stoken.expose_secret(), "v2_new_stoken");
        assert_eq!(account.mid, "mid_new");
        assert_eq!(account.note, "大号");
        assert_eq!(account.game_role, Some(role("旅行者")));
    }

    #[test]
    fn active_falls_back_to_the_first_account() {
        let (first, second) = (account("300012345"), account("300067890"));
        let (first_id, second_id) = (first.id.clone(), second.id.clone());
        let file = file_with(vec![first, second]);
        assert_eq!(file.active(Some(&second_id)).unwrap().id, second_id);
        assert_eq!(file.active(None).unwrap().id, first_id);
        assert_eq!(file.active(Some("gone")).unwrap().id, first_id);
        assert!(AccountFile::empty().active(None).is_none());
    }

    #[test]
    fn remove_deletes_only_that_account() {
        let (first, second) = (account("300012345"), account("300067890"));
        let first_id = first.id.clone();
        let mut file = file_with(vec![first, second]);
        assert_eq!(file.remove(&first_id).unwrap().uid, "300012345");
        assert!(file.remove(&first_id).is_none());
        assert_eq!(file.accounts.len(), 1);
        assert_eq!(file.accounts[0].uid, "300067890");
    }

    #[test]
    fn notes_are_cleaned_and_limited() {
        assert_eq!(normalize_note("  大号\n "), "大号");
        assert_eq!(normalize_note("a\u{202E}b"), "a b");
        assert_eq!(normalize_note(""), "");
        let long = "一二三四五六七八九十一二三四五六七八九十多出来";
        assert_eq!(normalize_note(long).chars().count(), MAX_NOTE_CHARS);
    }
}
