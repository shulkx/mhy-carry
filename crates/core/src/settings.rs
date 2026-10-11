//! 设置文件 `settings.json`。里面没有凭证，分享出去也没关系。

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::fsutil::{atomic_write, backup_path};

pub const SETTINGS_FORMAT: u32 = 1;

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Settings {
    pub format: u32,
    /// 首次运行时随机生成的 UUID v4，之后固定不变。
    pub device_id: String,
}

#[derive(Debug, thiserror::Error)]
pub enum SettingsError {
    #[error("读写设置文件失败（{path}）：{source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("生成设置文件内容失败：{0}")]
    Serialize(#[from] serde_json::Error),
}

impl Settings {
    /// 读取设置。文件不存在就生成新的设备 ID 并保存；
    /// 文件损坏就改名备份后重新生成（设置里没有凭证，最坏只是设备 ID 变了）。
    pub fn load_or_init(path: &Path) -> Result<Self, SettingsError> {
        let io_error = |source| SettingsError::Io {
            path: path.to_path_buf(),
            source,
        };
        // 按字节读：不是合法 UTF-8 的文件也当作损坏处理。
        match fs::read(path) {
            Ok(bytes) => match serde_json::from_slice::<Settings>(&bytes) {
                Ok(settings) => return Ok(settings),
                Err(_) => fs::rename(path, backup_path(path)).map_err(io_error)?,
            },
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(io_error(e)),
        }
        let settings = Settings {
            format: SETTINGS_FORMAT,
            device_id: uuid::Uuid::new_v4().to_string(),
        };
        settings.save(path)?;
        Ok(settings)
    }

    pub fn save(&self, path: &Path) -> Result<(), SettingsError> {
        let json = serde_json::to_string_pretty(self)?;
        atomic_write(path, json.as_bytes()).map_err(|source| SettingsError::Io {
            path: path.to_path_buf(),
            source,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn first_run_creates_a_device_id_and_keeps_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");

        let first = Settings::load_or_init(&path).unwrap();
        assert!(path.exists());
        assert_eq!(first.device_id.len(), 36);

        let second = Settings::load_or_init(&path).unwrap();
        assert_eq!(first.device_id, second.device_id);
    }

    #[test]
    fn corrupt_settings_are_backed_up_and_recreated() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        fs::write(&path, "garbage").unwrap();

        let settings = Settings::load_or_init(&path).unwrap();
        assert_eq!(settings.format, SETTINGS_FORMAT);

        let backups = fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .filter(|name| name.starts_with("settings.json.bak-"))
            .count();
        assert_eq!(backups, 1);
    }

    #[test]
    fn unknown_fields_from_newer_versions_are_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        fs::write(
            &path,
            r#"{"format":1,"device_id":"0b8f4a1e-1111-4222-8333-944445555666","scan":{"x":1}}"#,
        )
        .unwrap();
        let settings = Settings::load_or_init(&path).unwrap();
        assert_eq!(settings.device_id, "0b8f4a1e-1111-4222-8333-944445555666");
    }

    #[test]
    fn non_utf8_settings_are_backed_up_and_recreated() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        fs::write(&path, [0xff, 0xfe, 0x00, 0x7b]).unwrap();
        let settings = Settings::load_or_init(&path).unwrap();
        assert_eq!(settings.format, SETTINGS_FORMAT);
    }
}
