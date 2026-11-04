//! 安装版在注册表里的信息（设计文档第 6.6 节“更新”第 5 步）。

use std::path::Path;

use windows::Win32::Foundation::ERROR_FILE_NOT_FOUND;
use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, REG_SZ, RRF_RT_REG_SZ, RegGetValueW, RegSetKeyValueW,
};
use windows::core::HSTRING;

/// 安装包写的卸载信息。Tauri 的 NSIS 安装包只为当前用户安装，写在 HKCU 下。
const UNINSTALL_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Uninstall\MhyCarry";

/// 程序是安装版时，把“设置 → 应用”里显示的版本号改成 `version`，返回 true；
/// 免安装版什么都不改，返回 false。
pub fn record_version(exe_dir: &Path, version: &str) -> windows::core::Result<bool> {
    record_version_at(HKEY_CURRENT_USER, UNINSTALL_KEY, exe_dir, version)
}

fn record_version_at(
    root: HKEY,
    key: &str,
    exe_dir: &Path,
    version: &str,
) -> windows::core::Result<bool> {
    let Some(location) = read_string(root, key, "InstallLocation")? else {
        return Ok(false);
    };
    if !same_dir(&location, exe_dir) {
        return Ok(false);
    }
    write_string(root, key, "DisplayVersion", version)?;
    Ok(true)
}

/// 注册表里的路径带引号。比较时不分大小写，不管末尾的斜杠。
fn same_dir(location: &str, dir: &Path) -> bool {
    let clean = |text: &str| {
        text.trim()
            .trim_matches('"')
            .trim_end_matches(['\\', '/'])
            .to_lowercase()
    };
    let location = clean(location);
    !location.is_empty() && location == clean(&dir.to_string_lossy())
}

/// 读一个字符串值。键或值不存在时返回 `None`。
fn read_string(root: HKEY, key: &str, name: &str) -> windows::core::Result<Option<String>> {
    let key = HSTRING::from(key);
    let name = HSTRING::from(name);
    let mut size = 0u32;
    // 第一次只问长度（字节数，含结尾的 0）。
    let status = unsafe {
        RegGetValueW(
            root,
            &key,
            &name,
            RRF_RT_REG_SZ,
            None,
            None,
            Some(&mut size),
        )
    };
    if status == ERROR_FILE_NOT_FOUND {
        return Ok(None);
    }
    status.ok()?;
    let mut buffer = vec![0u16; (size as usize).div_ceil(2)];
    let status = unsafe {
        RegGetValueW(
            root,
            &key,
            &name,
            RRF_RT_REG_SZ,
            None,
            Some(buffer.as_mut_ptr().cast()),
            Some(&mut size),
        )
    };
    status.ok()?;
    let end = buffer
        .iter()
        .position(|&unit| unit == 0)
        .unwrap_or(buffer.len());
    Ok(Some(String::from_utf16_lossy(&buffer[..end])))
}

/// 写一个字符串值，键不存在时自动创建。
fn write_string(root: HKEY, key: &str, name: &str, value: &str) -> windows::core::Result<()> {
    let data: Vec<u16> = value.encode_utf16().chain(std::iter::once(0)).collect();
    let bytes = u32::try_from(data.len() * 2).unwrap_or(u32::MAX);
    unsafe {
        RegSetKeyValueW(
            root,
            &HSTRING::from(key),
            &HSTRING::from(name),
            REG_SZ.0,
            Some(data.as_ptr().cast()),
            bytes,
        )
    }
    .ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::System::Registry::RegDeleteTreeW;

    #[test]
    fn install_locations_are_compared_loosely() {
        let dir = Path::new(r"C:\Users\A\AppData\Local\MhyCarry");
        assert!(same_dir(r#""C:\Users\A\AppData\Local\MhyCarry""#, dir));
        assert!(same_dir(r"c:\users\a\appdata\local\mhycarry\", dir));
        assert!(!same_dir(r#""C:\Users\A\AppData\Local\Other""#, dir));
        assert!(!same_dir(r#""""#, Path::new("")));
    }

    /// 测试用的注册表键，测完删掉。
    struct TestKey(String);

    impl Drop for TestKey {
        fn drop(&mut self) {
            let _ = unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, &HSTRING::from(self.0.as_str())) };
        }
    }

    #[test]
    fn only_the_installed_copy_gets_the_new_version() {
        let key = TestKey(format!(r"Software\MhyCarryTest-{}", std::process::id()));
        write_string(
            HKEY_CURRENT_USER,
            &key.0,
            "InstallLocation",
            r#""C:\Apps\MhyCarry""#,
        )
        .unwrap();
        write_string(HKEY_CURRENT_USER, &key.0, "DisplayVersion", "0.4.0").unwrap();
        let version = || read_string(HKEY_CURRENT_USER, &key.0, "DisplayVersion").unwrap();

        let portable = Path::new(r"D:\Tools");
        assert!(!record_version_at(HKEY_CURRENT_USER, &key.0, portable, "0.4.1").unwrap());
        assert_eq!(version().as_deref(), Some("0.4.0"));

        let installed = Path::new(r"C:\Apps\MhyCarry");
        assert!(record_version_at(HKEY_CURRENT_USER, &key.0, installed, "0.4.1").unwrap());
        assert_eq!(version().as_deref(), Some("0.4.1"));

        let missing = format!(r"{}\missing", key.0);
        assert!(!record_version_at(HKEY_CURRENT_USER, &missing, installed, "0.4.1").unwrap());
    }
}
