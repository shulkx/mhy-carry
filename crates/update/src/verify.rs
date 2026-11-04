//! 核对更新文件（设计文档第 6.6 节“更新”第 1、2 步、“签名”）。

use minisign_verify::{PublicKey, Signature};

use crate::error::UpdateError;
use crate::release::program_file;
use crate::version::Version;

/// 发布签名用的公钥（minisign 格式）。私钥只在你的 1Password 和仓库的 Actions secrets 里；
/// `release.yml` 的 `sign` job 签名后也用这个文件核对一遍。
const OFFICIAL_KEY: &str = include_str!("../update.pub");

pub(crate) fn official_key() -> Result<PublicKey, UpdateError> {
    PublicKey::decode(OFFICIAL_KEY)
        .map_err(|_| UpdateError::Verify("程序里的公钥格式不对".to_owned()))
}

/// 签名里受签名保护的说明。`release.yml` 签名时写 `MhyCarry <tag>`；手动运行发布流程时
/// 写的是分支名，这样的签名冒充不了正式发布。
pub(crate) fn trusted_comment(version: Version) -> String {
    format!("MhyCarry v{version}")
}

/// 用 `key` 核对 `SHA256SUMS` 的签名，确认签名是给 `version` 的，再从里面取出这个版本
/// 免安装版 exe 的 SHA256（小写十六进制）。文件名里也有版本号，旧版本的文件列表冒充不了新版本。
pub(crate) fn expected_hash(
    key: &PublicKey,
    sums: &[u8],
    signature: &[u8],
    version: Version,
) -> Result<String, UpdateError> {
    let signature = check_signature(key, sums, signature)?;
    if signature.trusted_comment() != trusted_comment(version) {
        return Err(UpdateError::Verify(format!("签名不是给 v{version} 的")));
    }
    hash_of(sums, &program_file(version))
}

fn check_signature(
    key: &PublicKey,
    sums: &[u8],
    signature: &[u8],
) -> Result<Signature, UpdateError> {
    let fail = |reason: &str| UpdateError::Verify(reason.to_owned());
    let signature = std::str::from_utf8(signature)
        .ok()
        .and_then(|text| Signature::decode(text).ok())
        .ok_or_else(|| fail("签名文件格式不对"))?;
    // 只接受预先哈希的签名，这是 minisign 0.10 起的默认格式。
    key.verify(sums, &signature, false)
        .map_err(|_| fail("签名对不上，文件可能被换过"))?;
    Ok(signature)
}

/// 文件列表里 `file` 那一行的哈希。
fn hash_of(sums: &[u8], file: &str) -> Result<String, UpdateError> {
    let text = std::str::from_utf8(sums)
        .map_err(|_| UpdateError::Verify("文件列表不是 UTF-8".to_owned()))?;
    text.lines()
        .filter_map(|line| line.split_once("  "))
        .find(|(_, name)| *name == file)
        .map(|(hash, _)| hash)
        .filter(|hash| hash.len() == 64 && hash.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .map(str::to_ascii_lowercase)
        .ok_or_else(|| UpdateError::Verify(format!("签过名的文件列表里没有 {file}")))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::io::Cursor;

    /// 测试用的密钥。
    pub(crate) struct TestKey(minisign::KeyPair);

    impl TestKey {
        pub(crate) fn new() -> Self {
            Self(minisign::KeyPair::generate_unencrypted_keypair().unwrap())
        }

        pub(crate) fn public(&self) -> PublicKey {
            PublicKey::decode(&self.0.pk.to_box().unwrap().into_string()).unwrap()
        }

        /// 像 `release.yml` 一样签名，`comment` 是受签名保护的说明。
        pub(crate) fn sign(&self, data: &[u8], comment: &str) -> Vec<u8> {
            minisign::sign(
                Some(&self.0.pk),
                &self.0.sk,
                Cursor::new(data),
                Some(comment),
                None,
            )
            .unwrap()
            .into_string()
            .into_bytes()
        }
    }

    const V041: Version = Version {
        major: 0,
        minor: 4,
        patch: 1,
    };
    const HASH: &str = "28d6cafde291e409e21b6896a6ae7a0eda972c6a6a4aba3eb4af5b47a23be9da";
    const PROGRAM: &str = "MhyCarry_0.4.1_x64-portable.exe";
    const COMMENT: &str = "MhyCarry v0.4.1";

    fn sums() -> String {
        format!(
            "{HASH}  {PROGRAM}\nc1a8566b9d17a006bf7212a472f255f72fc65e43cd56137b0573c29a67ebaa04  MhyCarry_0.4.1_x64-setup.exe\n"
        )
    }

    fn check(key: &TestKey, sums: &str, signature: &[u8]) -> Result<String, UpdateError> {
        expected_hash(&key.public(), sums.as_bytes(), signature, V041)
    }

    #[test]
    fn signed_list_gives_the_program_hash() {
        let key = TestKey::new();
        let sums = sums();
        let signature = key.sign(sums.as_bytes(), COMMENT);
        assert_eq!(check(&key, &sums, &signature).unwrap(), HASH);
    }

    #[test]
    fn changed_lists_are_rejected() {
        let key = TestKey::new();
        let sums = sums();
        let signature = key.sign(sums.as_bytes(), COMMENT);
        let changed = sums.replacen("28d6", "0000", 1);
        let result = check(&key, &changed, &signature);
        assert!(matches!(result, Err(UpdateError::Verify(_))), "{result:?}");
    }

    #[test]
    fn other_keys_are_rejected() {
        let sums = sums();
        let signature = TestKey::new().sign(sums.as_bytes(), COMMENT);
        let result = check(&TestKey::new(), &sums, &signature);
        assert!(matches!(result, Err(UpdateError::Verify(_))), "{result:?}");
    }

    #[test]
    fn signatures_for_other_versions_or_branches_are_rejected() {
        let key = TestKey::new();
        let sums = sums();
        // 手动运行发布流程时，说明里是分支名。
        for comment in ["MhyCarry main", "MhyCarry v0.4.0", "MhyCarry v0.4.1 ", ""] {
            let result = check(&key, &sums, &key.sign(sums.as_bytes(), comment));
            assert!(
                matches!(&result, Err(UpdateError::Verify(reason)) if reason.contains("v0.4.1")),
                "{comment}: {result:?}"
            );
        }
    }

    #[test]
    fn an_old_signed_list_cannot_install_a_newer_version() {
        let key = TestKey::new();
        let old = sums().replace("0.4.1", "0.4.0");
        let result = check(&key, &old, &key.sign(old.as_bytes(), "MhyCarry v0.4.0"));
        assert!(matches!(result, Err(UpdateError::Verify(_))), "{result:?}");
        // 就算说明被写成了新版本，文件列表里也没有新版本的文件。
        let result = check(&key, &old, &key.sign(old.as_bytes(), COMMENT));
        assert!(
            matches!(&result, Err(UpdateError::Verify(reason)) if reason.contains(PROGRAM)),
            "{result:?}"
        );
    }

    #[test]
    fn garbage_signatures_are_rejected() {
        let key = TestKey::new();
        for signature in [
            &b""[..],
            b"untrusted comment: x\nnot base64\n",
            &[0xff, 0xfe],
        ] {
            let result = check(&key, &sums(), signature);
            assert!(matches!(result, Err(UpdateError::Verify(_))), "{result:?}");
        }
    }

    #[test]
    fn the_official_key_is_valid() {
        official_key().unwrap();
    }

    /// 核对 CI 生成的签名：先 `gh run download` 下载手动运行发布流程的 `dist` 和 `signature`
    /// 两个产物到同一个文件夹，再把文件夹路径放进 `MHY_CARRY_SIGNED_DIR`。手动运行时签名的说明
    /// 是分支名，所以这里不核对说明，只打印出来。
    #[test]
    #[ignore = "需要 CI 生成的签名文件"]
    fn verifies_a_signature_made_in_ci() {
        let dir = std::path::PathBuf::from(
            std::env::var("MHY_CARRY_SIGNED_DIR").expect("设置 MHY_CARRY_SIGNED_DIR"),
        );
        let sums = std::fs::read(dir.join("SHA256SUMS")).unwrap();
        let signature = std::fs::read(dir.join("SHA256SUMS.minisig")).unwrap();
        let text = String::from_utf8(sums.clone()).unwrap();
        let program = text
            .lines()
            .filter_map(|line| line.split_once("  "))
            .map(|(_, name)| name)
            .find(|name| name.ends_with("_x64-portable.exe"))
            .expect("文件列表里有免安装版");
        let signature = check_signature(&official_key().unwrap(), &sums, &signature).unwrap();
        let hash = hash_of(&sums, program).unwrap();
        println!(
            "签名正确：{program} {hash}，说明：{}",
            signature.trusted_comment()
        );
    }
}
