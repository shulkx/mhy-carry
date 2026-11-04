//! 核对更新文件（设计文档第 6.6 节“更新”第 1、2 步、“签名”）。

use minisign_verify::{PublicKey, Signature};

use crate::error::UpdateError;

/// 发布签名用的公钥（minisign 格式）。私钥只在你本机和仓库的 Actions secrets 里；
/// `release.yml` 的 `sign` job 签名后也用这个文件核对一遍。
const OFFICIAL_KEY: &str = include_str!("../update.pub");

pub(crate) fn official_key() -> Result<PublicKey, UpdateError> {
    PublicKey::decode(OFFICIAL_KEY)
        .map_err(|_| UpdateError::Verify("程序里的公钥格式不对".to_owned()))
}

/// 用 `key` 核对 `SHA256SUMS` 的签名，再从里面取出 `file` 的 SHA256（小写十六进制）。
/// 文件名里有版本号，所以旧版本的签名冒充不了新版本。
pub(crate) fn expected_hash(
    key: &PublicKey,
    sums: &[u8],
    signature: &[u8],
    file: &str,
) -> Result<String, UpdateError> {
    let fail = |reason: &str| UpdateError::Verify(reason.to_owned());
    let signature = std::str::from_utf8(signature)
        .ok()
        .and_then(|text| Signature::decode(text).ok())
        .ok_or_else(|| fail("签名文件格式不对"))?;
    // 只接受预先哈希的签名，这是 minisign 0.10 起的默认格式。
    key.verify(sums, &signature, false)
        .map_err(|_| fail("签名对不上，文件可能被换过"))?;
    let text = std::str::from_utf8(sums).map_err(|_| fail("文件列表不是 UTF-8"))?;
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

        pub(crate) fn sign(&self, data: &[u8]) -> Vec<u8> {
            minisign::sign(Some(&self.0.pk), &self.0.sk, Cursor::new(data), None, None)
                .unwrap()
                .into_string()
                .into_bytes()
        }
    }

    const HASH: &str = "28d6cafde291e409e21b6896a6ae7a0eda972c6a6a4aba3eb4af5b47a23be9da";
    const PROGRAM: &str = "MhyCarry_0.4.1_x64-portable.exe";

    fn sums() -> String {
        format!(
            "{HASH}  {PROGRAM}\nc1a8566b9d17a006bf7212a472f255f72fc65e43cd56137b0573c29a67ebaa04  MhyCarry_0.4.1_x64-setup.exe\n"
        )
    }

    #[test]
    fn signed_list_gives_the_program_hash() {
        let key = TestKey::new();
        let sums = sums();
        let hash = expected_hash(
            &key.public(),
            sums.as_bytes(),
            &key.sign(sums.as_bytes()),
            PROGRAM,
        );
        assert_eq!(hash.unwrap(), HASH);
    }

    #[test]
    fn changed_lists_are_rejected() {
        let key = TestKey::new();
        let sums = sums();
        let signature = key.sign(sums.as_bytes());
        let changed = sums.replacen("28d6", "0000", 1);
        let result = expected_hash(&key.public(), changed.as_bytes(), &signature, PROGRAM);
        assert!(matches!(result, Err(UpdateError::Verify(_))), "{result:?}");
    }

    #[test]
    fn other_keys_are_rejected() {
        let sums = sums();
        let signature = TestKey::new().sign(sums.as_bytes());
        let result = expected_hash(
            &TestKey::new().public(),
            sums.as_bytes(),
            &signature,
            PROGRAM,
        );
        assert!(matches!(result, Err(UpdateError::Verify(_))), "{result:?}");
    }

    #[test]
    fn an_old_signed_list_cannot_install_a_newer_version() {
        let key = TestKey::new();
        let old = sums().replace("0.4.1", "0.4.0");
        let result = expected_hash(
            &key.public(),
            old.as_bytes(),
            &key.sign(old.as_bytes()),
            PROGRAM,
        );
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
            let result = expected_hash(&key.public(), sums().as_bytes(), signature, PROGRAM);
            assert!(matches!(result, Err(UpdateError::Verify(_))), "{result:?}");
        }
    }

    #[test]
    fn the_official_key_is_valid() {
        official_key().unwrap();
    }

    /// 核对 CI 生成的签名：先 `gh run download` 下载手动运行发布流程的 `dist` 和 `signature`
    /// 两个产物到同一个文件夹，再把文件夹路径放进 `MHY_CARRY_SIGNED_DIR`。
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
        let hash = expected_hash(&official_key().unwrap(), &sums, &signature, program).unwrap();
        println!("签名正确：{program} {hash}");
    }
}
