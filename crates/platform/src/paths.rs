//! 数据目录：`%LOCALAPPDATA%\MhyCarry\`。

use std::ffi::OsString;
use std::path::PathBuf;

/// 设置后用它作为数据目录，开发调试时用来避开真实账号。
pub const DATA_DIR_ENV: &str = "MHY_CARRY_DATA_DIR";
const APP_DIR_NAME: &str = "MhyCarry";

#[derive(Debug, thiserror::Error)]
#[error("找不到系统的本地应用数据目录（%LOCALAPPDATA%）")]
pub struct NoDataDir;

pub fn data_dir() -> Result<PathBuf, NoDataDir> {
    resolve(std::env::var_os(DATA_DIR_ENV), dirs::data_local_dir())
}

pub fn accounts_path() -> Result<PathBuf, NoDataDir> {
    Ok(data_dir()?.join("accounts.json"))
}

pub fn settings_path() -> Result<PathBuf, NoDataDir> {
    Ok(data_dir()?.join("settings.json"))
}

fn resolve(
    env_override: Option<OsString>,
    local_data: Option<PathBuf>,
) -> Result<PathBuf, NoDataDir> {
    if let Some(dir) = env_override.filter(|v| !v.is_empty()) {
        return Ok(PathBuf::from(dir));
    }
    local_data.map(|d| d.join(APP_DIR_NAME)).ok_or(NoDataDir)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uses_local_app_data_by_default() {
        let dir = resolve(None, Some(PathBuf::from(r"C:\Users\a\AppData\Local"))).unwrap();
        assert_eq!(
            dir,
            PathBuf::from(r"C:\Users\a\AppData\Local").join("MhyCarry")
        );
    }

    #[test]
    fn env_override_wins() {
        let dir = resolve(
            Some(OsString::from(r"D:\tmp\mhy")),
            Some(PathBuf::from(r"C:\Users\a\AppData\Local")),
        )
        .unwrap();
        assert_eq!(dir, PathBuf::from(r"D:\tmp\mhy"));
    }

    #[test]
    fn empty_env_override_is_ignored() {
        let dir = resolve(Some(OsString::new()), Some(PathBuf::from("local"))).unwrap();
        assert_eq!(dir, PathBuf::from("local").join("MhyCarry"));
    }

    #[test]
    fn errors_when_no_directory_is_known() {
        assert!(resolve(None, None).is_err());
    }
}
