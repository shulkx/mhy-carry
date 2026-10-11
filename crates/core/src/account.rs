//! 账号文件 `accounts.json` 的数据结构。v1 明文保存，格式见设计文档第 8 节。

pub use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize, Serializer};

/// 本程序写出的账号文件格式版本。
pub const ACCOUNT_FILE_FORMAT: u32 = 1;
/// v1 不加密。
pub const ENCRYPTION_NONE: &str = "none";

#[derive(Debug, Serialize, Deserialize)]
pub struct AccountFile {
    pub format: u32,
    pub encryption: String,
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
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Account {
    /// 本地生成的 UUID，只用来区分账号。
    pub id: String,
    pub uid: String,
    pub mid: String,
    #[serde(serialize_with = "serialize_secret")]
    pub stoken: SecretString,
    #[serde(default)]
    pub note: String,
    pub bound_at: jiff::Timestamp,
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
        }
    }

    pub fn masked_uid(&self) -> String {
        mask_uid(&self.uid)
    }
}

/// 只保留第一位和最后三位，中间固定显示 4 个星号，不暴露 uid 的长度。
pub fn mask_uid(uid: &str) -> String {
    let chars: Vec<char> = uid.chars().collect();
    if chars.len() <= 4 {
        return "****".to_owned();
    }
    let first = chars[0];
    let last3: String = chars[chars.len() - 3..].iter().collect();
    format!("{first}****{last3}")
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

    fn sample_account() -> Account {
        Account::new(
            "123456789".to_owned(),
            "mid_abc".to_owned(),
            SecretString::from("v2_secret_stoken"),
            "2026-10-11T08:00:00Z".parse().unwrap(),
        )
    }

    #[test]
    fn masks_uid_keeping_first_and_last_three() {
        assert_eq!(mask_uid("123456789"), "1****789");
        assert_eq!(mask_uid("12345"), "1****345");
        assert_eq!(mask_uid("1234"), "****");
        assert_eq!(mask_uid(""), "****");
    }

    #[test]
    fn debug_output_never_contains_the_stoken() {
        let file = AccountFile {
            accounts: vec![sample_account()],
            ..AccountFile::empty()
        };
        let debug = format!("{file:?}");
        assert!(!debug.contains("v2_secret_stoken"), "{debug}");
    }

    #[test]
    fn round_trips_through_json() {
        let file = AccountFile {
            accounts: vec![sample_account()],
            ..AccountFile::empty()
        };
        let json = serde_json::to_string(&file).unwrap();
        assert!(json.contains(r#""format":1"#), "{json}");
        assert!(json.contains(r#""encryption":"none""#), "{json}");
        assert!(json.contains(r#""stoken":"v2_secret_stoken""#), "{json}");
        assert!(
            json.contains(r#""bound_at":"2026-10-11T08:00:00Z""#),
            "{json}"
        );

        let back: AccountFile = serde_json::from_str(&json).unwrap();
        let account = &back.accounts[0];
        assert_eq!(account.uid, "123456789");
        assert_eq!(account.mid, "mid_abc");
        assert_eq!(account.stoken.expose_secret(), "v2_secret_stoken");
        assert_eq!(account.note, "");
    }

    #[test]
    fn new_accounts_get_distinct_ids() {
        assert_ne!(sample_account().id, sample_account().id);
    }
}
