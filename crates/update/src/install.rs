//! 换掉正在运行的程序（设计文档第 6.6 节“更新”第 4、6 步）。Windows 不能删除正在运行的
//! exe，但可以改名：旧的改名为 `.old`，新的放到原来的位置。

use std::ffi::OsString;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// 下载中的新版本：`<程序文件名>.new`。
pub fn staged_path(exe: &Path) -> PathBuf {
    with_suffix(exe, ".new")
}

/// 换下来的旧版本：`<程序文件名>.old`。
pub fn old_path(exe: &Path) -> PathBuf {
    with_suffix(exe, ".old")
}

fn with_suffix(exe: &Path, suffix: &str) -> PathBuf {
    let mut name = OsString::from(exe.as_os_str());
    name.push(suffix);
    PathBuf::from(name)
}

/// 用 `.new` 换掉 `exe`。先删掉上次留下的 `.old`；`.new` 放不到原位置时，把旧的改回来。
pub fn swap(exe: &Path) -> io::Result<()> {
    let staged = staged_path(exe);
    let old = old_path(exe);
    remove_if_present(&old)?;
    fs::rename(exe, &old)?;
    if let Err(error) = fs::rename(&staged, exe) {
        let _ = fs::rename(&old, exe);
        return Err(error);
    }
    Ok(())
}

/// 删掉上次更新留下的 `.old` 和没下载完的 `.new`，返回是否都删干净了。
/// 旧进程可能还没退出，删不掉时调用方过一会儿再试。
pub fn clean_up(exe: &Path) -> bool {
    [old_path(exe), staged_path(exe)]
        .iter()
        .all(|path| remove_if_present(path).is_ok())
}

fn remove_if_present(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn program() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("MhyCarry_0.4.0_x64-portable.exe");
        fs::write(&exe, "旧版本").unwrap();
        (dir, exe)
    }

    fn read(path: &Path) -> String {
        fs::read_to_string(path).unwrap()
    }

    #[test]
    fn swap_puts_the_new_program_in_place() {
        let (_dir, exe) = program();
        fs::write(staged_path(&exe), "新版本").unwrap();
        swap(&exe).unwrap();
        assert_eq!(read(&exe), "新版本");
        assert_eq!(read(&old_path(&exe)), "旧版本");
        assert!(!staged_path(&exe).exists());
    }

    #[test]
    fn swap_replaces_a_leftover_old_file() {
        let (_dir, exe) = program();
        fs::write(old_path(&exe), "更旧的版本").unwrap();
        fs::write(staged_path(&exe), "新版本").unwrap();
        swap(&exe).unwrap();
        assert_eq!(read(&old_path(&exe)), "旧版本");
    }

    #[test]
    fn failed_swap_restores_the_old_program() {
        let (_dir, exe) = program();
        assert!(swap(&exe).is_err());
        assert_eq!(read(&exe), "旧版本");
        assert!(!old_path(&exe).exists());
    }

    #[test]
    fn swap_changes_nothing_when_the_old_file_cannot_be_removed() {
        let (_dir, exe) = program();
        fs::write(staged_path(&exe), "新版本").unwrap();
        // 删不掉的 `.old`：一个同名的文件夹。
        fs::create_dir(old_path(&exe)).unwrap();
        assert!(swap(&exe).is_err());
        assert_eq!(read(&exe), "旧版本");
        assert_eq!(read(&staged_path(&exe)), "新版本");
    }

    #[test]
    fn clean_up_removes_leftovers() {
        let (_dir, exe) = program();
        fs::write(old_path(&exe), "旧版本").unwrap();
        fs::write(staged_path(&exe), "没下完").unwrap();
        assert!(clean_up(&exe));
        assert!(!old_path(&exe).exists());
        assert!(!staged_path(&exe).exists());
        assert_eq!(read(&exe), "旧版本");
        assert!(clean_up(&exe));
    }
}
