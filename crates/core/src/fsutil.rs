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
    tmp_name.push(".tmp");
    let tmp = dir.join(tmp_name);
    {
        let mut file = File::create(&tmp)?;
        file.write_all(contents)?;
        file.sync_all()?;
    }
    // Windows 上 std::fs::rename 会直接替换已存在的目标文件。
    fs::rename(&tmp, path)
}

/// 损坏文件的备份路径：同目录下的 `<原文件名>.bak-<Unix 秒>`。
pub fn backup_path(path: &Path) -> PathBuf {
    let mut name: OsString = path.file_name().unwrap_or_default().to_os_string();
    name.push(format!(".bak-{}", jiff::Timestamp::now().as_second()));
    path.with_file_name(name)
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
    fn backup_path_keeps_directory_and_adds_suffix() {
        let path = Path::new("dir").join("accounts.json");
        let backup = backup_path(&path);
        assert_eq!(backup.parent(), path.parent());
        let name = backup.file_name().unwrap().to_str().unwrap();
        assert!(name.starts_with("accounts.json.bak-"), "{name}");
    }
}
