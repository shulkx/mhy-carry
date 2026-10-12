//! 账号文件的读写。v1 只有明文 JSON 一种实现，以后加密时再加一种实现。

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::account::{ACCOUNT_FILE_FORMAT, AccountFile, ENCRYPTION_NONE};
use crate::fsutil::{atomic_write, backup_path, leftovers, strip_bom};

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("读写账号文件失败（{path}）：{source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("账号文件已损坏，已改名为 {backup}，请重新绑定账号")]
    Corrupt { backup: PathBuf },
    #[error(
        "账号文件来自更新版本的 MhyCarry（format={format}，encryption={encryption}），为免损坏，本程序不会修改它"
    )]
    UnsupportedFormat { format: u32, encryption: String },
    #[error("生成账号文件内容失败：{0}")]
    Serialize(#[from] serde_json::Error),
}

pub trait AccountStore {
    fn load(&self) -> Result<AccountFile, StoreError>;
    fn save(&self, file: &AccountFile) -> Result<(), StoreError>;
}

/// 明文 JSON 文件。
pub struct JsonFileStore {
    path: PathBuf,
}

/// 只读文件头，判断格式版本和加密方式。
#[derive(Deserialize)]
struct Header {
    format: u32,
    /// 以后的格式里它不一定还是字符串，先按任意 JSON 值读。
    #[serde(default)]
    encryption: serde_json::Value,
}

enum OnDisk {
    Missing,
    Supported(Vec<u8>),
    Unsupported { format: u32, encryption: String },
    Unparsable,
}

impl JsonFileStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// 同目录下的损坏备份和没删掉的临时文件。它们是旧的账号文件，里面可能有明文凭证，
    /// 解绑时要一并删除。
    pub fn leftover_files(&self) -> Result<Vec<PathBuf>, StoreError> {
        leftovers(&self.path).map_err(|e| self.io_error(e))
    }

    fn io_error(&self, source: io::Error) -> StoreError {
        StoreError::Io {
            path: self.path.clone(),
            source,
        }
    }

    fn read_on_disk(&self) -> Result<OnDisk, StoreError> {
        // 按字节读：不是合法 UTF-8 的文件也要走“损坏”分支，不能变成读写都失败。
        let raw = match fs::read(&self.path) {
            Ok(raw) => raw,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(OnDisk::Missing),
            Err(e) => return Err(self.io_error(e)),
        };
        let bytes = strip_bom(&raw);
        let Ok(header) = serde_json::from_slice::<Header>(bytes) else {
            return Ok(OnDisk::Unparsable);
        };
        let encryption = match &header.encryption {
            serde_json::Value::String(s) => s.clone(),
            other => other.to_string(),
        };
        // 格式比本程序新时，不管其他字段长什么样都只读（设计文档第 8 节）。
        if header.format > ACCOUNT_FILE_FORMAT {
            return Ok(OnDisk::Unsupported {
                format: header.format,
                encryption,
            });
        }
        Ok(match header.encryption.as_str() {
            Some(ENCRYPTION_NONE) => OnDisk::Supported(bytes.to_vec()),
            Some(_) => OnDisk::Unsupported {
                format: header.format,
                encryption,
            },
            // 本程序自己的格式却没有 encryption 字段，当作损坏。
            None => OnDisk::Unparsable,
        })
    }

    fn back_up_corrupt_file(&self) -> Result<PathBuf, StoreError> {
        let backup = backup_path(&self.path);
        fs::rename(&self.path, &backup).map_err(|e| self.io_error(e))?;
        Ok(backup)
    }
}

impl AccountStore for JsonFileStore {
    fn load(&self) -> Result<AccountFile, StoreError> {
        match self.read_on_disk()? {
            OnDisk::Missing => Ok(AccountFile::empty()),
            OnDisk::Unsupported { format, encryption } => {
                Err(StoreError::UnsupportedFormat { format, encryption })
            }
            OnDisk::Unparsable => Err(StoreError::Corrupt {
                backup: self.back_up_corrupt_file()?,
            }),
            OnDisk::Supported(bytes) => match serde_json::from_slice(&bytes) {
                Ok(file) => Ok(file),
                Err(_) => Err(StoreError::Corrupt {
                    backup: self.back_up_corrupt_file()?,
                }),
            },
        }
    }

    fn save(&self, file: &AccountFile) -> Result<(), StoreError> {
        match self.read_on_disk()? {
            OnDisk::Unsupported { format, encryption } => {
                return Err(StoreError::UnsupportedFormat { format, encryption });
            }
            OnDisk::Unparsable => {
                self.back_up_corrupt_file()?;
            }
            OnDisk::Missing | OnDisk::Supported(_) => {}
        }
        let json = serde_json::to_string_pretty(file)?;
        atomic_write(&self.path, json.as_bytes()).map_err(|e| self.io_error(e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::account::{Account, SecretString};
    use std::fs;

    fn store_in(dir: &tempfile::TempDir) -> JsonFileStore {
        JsonFileStore::new(dir.path().join("accounts.json"))
    }

    fn file_with_one_account() -> AccountFile {
        let account = Account::new(
            "123456789".to_owned(),
            "mid_abc".to_owned(),
            SecretString::from("v2_secret_stoken"),
            "2026-10-11T08:00:00Z".parse().unwrap(),
        );
        AccountFile {
            accounts: vec![account],
            ..AccountFile::empty()
        }
    }

    #[test]
    fn missing_file_loads_as_empty() {
        let dir = tempfile::tempdir().unwrap();
        let file = store_in(&dir).load().unwrap();
        assert!(file.accounts.is_empty());
        assert_eq!(file.format, ACCOUNT_FILE_FORMAT);
    }

    #[test]
    fn save_then_load_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let store = store_in(&dir);
        store.save(&file_with_one_account()).unwrap();
        let loaded = store.load().unwrap();
        assert_eq!(loaded.accounts.len(), 1);
        assert_eq!(loaded.accounts[0].uid, "123456789");
    }

    #[test]
    fn corrupt_file_is_renamed_not_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let store = store_in(&dir);
        fs::write(store.path(), "{ not json").unwrap();

        let backup = match store.load() {
            Err(StoreError::Corrupt { backup }) => backup,
            other => panic!("应该是 Corrupt，实际是 {other:?}"),
        };
        assert!(!store.path().exists());
        assert_eq!(fs::read_to_string(backup).unwrap(), "{ not json");
    }

    #[test]
    fn valid_header_but_broken_body_counts_as_corrupt() {
        let dir = tempfile::tempdir().unwrap();
        let store = store_in(&dir);
        fs::write(
            store.path(),
            r#"{"format":1,"encryption":"none","accounts":[{"uid":1}]}"#,
        )
        .unwrap();
        assert!(matches!(store.load(), Err(StoreError::Corrupt { .. })));
    }

    #[test]
    fn newer_format_is_read_only() {
        let dir = tempfile::tempdir().unwrap();
        let store = store_in(&dir);
        let newer = r#"{"format":2,"encryption":"dpapi","accounts":"opaque"}"#;
        fs::write(store.path(), newer).unwrap();

        assert!(matches!(
            store.load(),
            Err(StoreError::UnsupportedFormat { format: 2, .. })
        ));
        assert!(matches!(
            store.save(&AccountFile::empty()),
            Err(StoreError::UnsupportedFormat { .. })
        ));
        assert_eq!(fs::read_to_string(store.path()).unwrap(), newer);
    }

    #[test]
    fn unknown_encryption_is_read_only() {
        let dir = tempfile::tempdir().unwrap();
        let store = store_in(&dir);
        fs::write(
            store.path(),
            r#"{"format":1,"encryption":"magic","accounts":[]}"#,
        )
        .unwrap();
        assert!(matches!(
            store.load(),
            Err(StoreError::UnsupportedFormat { .. })
        ));
    }

    #[test]
    fn newer_format_is_read_only_whatever_the_other_header_fields_look_like() {
        for newer in [
            r#"{"format":2}"#,
            r#"{"format":2,"encryption":{"kind":"dpapi"}}"#,
        ] {
            let dir = tempfile::tempdir().unwrap();
            let store = store_in(&dir);
            fs::write(store.path(), newer).unwrap();
            assert!(
                matches!(
                    store.load(),
                    Err(StoreError::UnsupportedFormat { format: 2, .. })
                ),
                "{newer}"
            );
            assert!(
                matches!(
                    store.save(&AccountFile::empty()),
                    Err(StoreError::UnsupportedFormat { .. })
                ),
                "{newer}"
            );
            assert_eq!(fs::read_to_string(store.path()).unwrap(), newer);
        }
    }

    #[test]
    fn utf8_bom_is_accepted() {
        let dir = tempfile::tempdir().unwrap();
        let store = store_in(&dir);
        store.save(&file_with_one_account()).unwrap();
        let mut bytes = b"\xEF\xBB\xBF".to_vec();
        bytes.extend(fs::read(store.path()).unwrap());
        fs::write(store.path(), bytes).unwrap();
        assert_eq!(store.load().unwrap().accounts.len(), 1);
    }

    #[test]
    fn lists_backups_left_by_a_corrupt_file() {
        let dir = tempfile::tempdir().unwrap();
        let store = store_in(&dir);
        assert!(store.leftover_files().unwrap().is_empty());
        fs::write(store.path(), "garbage").unwrap();
        assert!(store.load().is_err());
        assert_eq!(store.leftover_files().unwrap().len(), 1);
    }

    #[test]
    fn saving_over_a_corrupt_file_backs_it_up_first() {
        let dir = tempfile::tempdir().unwrap();
        let store = store_in(&dir);
        fs::write(store.path(), "garbage").unwrap();

        store.save(&AccountFile::empty()).unwrap();

        let backups = fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .filter(|name| name.starts_with("accounts.json.bak-"))
            .count();
        assert_eq!(backups, 1);
        assert!(store.load().unwrap().accounts.is_empty());
    }

    #[test]
    fn non_utf8_file_counts_as_corrupt() {
        let dir = tempfile::tempdir().unwrap();
        let store = store_in(&dir);
        fs::write(store.path(), [0xff, 0xfe, 0x00, 0x7b]).unwrap();
        assert!(matches!(store.load(), Err(StoreError::Corrupt { .. })));
    }
}
