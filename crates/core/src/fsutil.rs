//! 文件读写的小工具。

use std::ffi::OsString;
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// 原子写入：先在同目录写临时文件并刷到磁盘，再替换目标文件。
/// 写到一半断电时，目标文件要么是旧内容，要么是新内容，不会只剩半截。
pub fn atomic_write(path: &Path, contents: &[u8]) -> io::Result<()> {
    let file_name = path
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "路径里没有文件名"))?;
    let dir = match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => Path::new("."),
    };
    fs::create_dir_all(dir)?;

    let mut tmp_name = file_name.to_os_string();
    tmp_name.push(TMP_SUFFIX);
    let tmp = dir.join(tmp_name);
    let result = File::create(&tmp)
        .and_then(|mut file| {
            file.write_all(contents)?;
            file.sync_all()
        })
        // Windows 上 std::fs::rename 会直接替换已存在的目标文件。
        .and_then(|()| fs::rename(&tmp, path));
    if result.is_err() {
        // 临时文件里可能就是明文凭证，失败了也不留下。
        let _ = fs::remove_file(&tmp);
    }
    result
}

const TMP_SUFFIX: &str = ".tmp";
const BACKUP_INFIX: &str = ".bak-";

/// 损坏文件的备份路径：同目录下的 `<原文件名>.bak-<Unix 秒>`。
pub fn backup_path(path: &Path) -> PathBuf {
    let mut name: OsString = path.file_name().unwrap_or_default().to_os_string();
    name.push(format!(
        "{BACKUP_INFIX}{}",
        jiff::Timestamp::now().as_second()
    ));
    path.with_file_name(name)
}

/// 同目录下留下的损坏备份（`<原文件名>.bak-*`）和没删掉的临时文件（`<原文件名>.tmp`）。
/// 目录不存在时返回空列表。
pub fn leftovers(path: &Path) -> io::Result<Vec<PathBuf>> {
    let (Some(dir), Some(name)) = (path.parent(), path.file_name().and_then(|n| n.to_str())) else {
        return Ok(Vec::new());
    };
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    let backup_prefix = format!("{name}{BACKUP_INFIX}");
    let tmp_name = format!("{name}{TMP_SUFFIX}");
    let mut found = Vec::new();
    for entry in entries {
        let entry = entry?;
        let file_name = entry.file_name();
        let Some(file_name) = file_name.to_str() else {
            continue;
        };
        if file_name.starts_with(&backup_prefix) || file_name == tmp_name {
            found.push(entry.path());
        }
    }
    Ok(found)
}

/// 去掉文件开头的 UTF-8 BOM。记事本选“UTF-8 带 BOM”保存时会加上它，JSON 解析不认。
pub fn strip_bom(bytes: &[u8]) -> &[u8] {
    bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn creates_missing_parent_directories() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a").join("b").join("data.json");
        atomic_write(&path, b"hello").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"hello");
    }

    #[test]
    fn replaces_existing_file_and_leaves_no_temp_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("data.json");
        atomic_write(&path, b"old").unwrap();
        atomic_write(&path, b"new").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"new");
        let names: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, vec!["data.json"]);
    }

    #[test]
    fn failed_write_leaves_no_temp_file() {
        let dir = tempfile::tempdir().unwrap();
        // 目标位置是个目录，替换一定失败。
        let path = dir.path().join("data.json");
        fs::create_dir(&path).unwrap();
        assert!(atomic_write(&path, b"secret").is_err());
        assert!(!dir.path().join("data.json.tmp").exists());
    }

    #[test]
    fn finds_leftover_backups_and_temp_files() {
        let dir = tempfile::tempdir().unwrap();
        for name in [
            "accounts.json",
            "accounts.json.bak-1",
            "accounts.json.bak-2",
            "accounts.json.tmp",
            "accounts.json.old",
            "settings.json.bak-1",
        ] {
            fs::write(dir.path().join(name), "x").unwrap();
        }
        let mut found: Vec<_> = leftovers(&dir.path().join("accounts.json"))
            .unwrap()
            .into_iter()
            .map(|p| p.file_name().unwrap().to_owned())
            .collect();
        found.sort();
        assert_eq!(
            found,
            [
                "accounts.json.bak-1",
                "accounts.json.bak-2",
                "accounts.json.tmp"
            ]
        );
    }

    #[test]
    fn no_leftovers_when_the_directory_is_missing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("missing").join("accounts.json");
        assert!(leftovers(&path).unwrap().is_empty());
    }

    #[test]
    fn strips_utf8_bom() {
        assert_eq!(strip_bom(b"\xEF\xBB\xBF{}"), b"{}");
        assert_eq!(strip_bom(b"{}"), b"{}");
    }

    #[test]
    fn backup_path_keeps_directory_and_adds_suffix() {
        let path = Path::new("dir").join("accounts.json");
        let backup = backup_path(&path);
        assert_eq!(backup.parent(), path.parent());
        let name = backup.file_name().unwrap().to_str().unwrap();
        assert!(name.starts_with("accounts.json.bak-"), "{name}");
    }
}
