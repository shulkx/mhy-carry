//! 检查更新和自动更新（设计文档第 6.6 节）：查 GitHub 上的最新发布，下载、核对新版本，
//! 换掉正在运行的程序。不依赖其他单元，也不依赖 Windows。
#![forbid(unsafe_code)]

mod client;
mod error;
pub mod install;
mod release;
mod verify;
mod version;

use std::path::{Path, PathBuf};

use minisign_verify::PublicKey;

pub use client::UpdateClient;
pub use error::UpdateError;
pub use release::{Release, release_page};
pub use version::Version;

/// 查有没有比 `current` 新的正式版本（第 6.6 节“检查”）。有新版本时，再下载它的文件列表和签名
/// 核对一遍：对得上才能自动更新；没有签名（v0.3.1 和更早的版本）或者对不上，只能到发布页面
/// 手动下载。
pub async fn check(
    client: &UpdateClient,
    current: Version,
) -> Result<Option<Release>, UpdateError> {
    check_with(client, &verify::official_key()?, current).await
}

async fn check_with(
    client: &UpdateClient,
    key: &PublicKey,
    current: Version,
) -> Result<Option<Release>, UpdateError> {
    let version = client.latest().await?;
    if version <= current {
        return Ok(None);
    }
    let installable = match signed_hash(client, key, version).await {
        Ok(_) => true,
        Err(UpdateError::Status(404) | UpdateError::Verify(_)) => false,
        Err(error) => return Err(error),
    };
    Ok(Some(Release {
        version,
        installable,
    }))
}

/// 下载 `version` 的文件列表和签名，核对后返回免安装版 exe 的 SHA256。
async fn signed_hash(
    client: &UpdateClient,
    key: &PublicKey,
    version: Version,
) -> Result<String, UpdateError> {
    let sums = client
        .fetch(version, release::SUMS, client::MAX_SMALL)
        .await?;
    let signature = client
        .fetch(version, release::SUMS_SIGNATURE, client::MAX_SMALL)
        .await?;
    verify::expected_hash(key, &sums, &signature, version)
}

/// 下载并核对好的新版本，放在程序旁边的 `.new`。
#[derive(Debug)]
pub struct Staged {
    exe: PathBuf,
    hash: String,
}

impl Staged {
    /// 再读一遍 `.new` 核对哈希，然后换掉正在运行的程序（第 6.6 节“更新”第 4 步）。
    pub fn install(&self) -> Result<(), UpdateError> {
        let staged = install::staged_path(&self.exe);
        if client::hash_file(&staged).map_err(UpdateError::Replace)? != self.hash {
            return Err(UpdateError::Verify("新版本的文件在替换前被改过".to_owned()));
        }
        install::swap(&self.exe).map_err(UpdateError::Replace)
    }
}

/// 下载并核对 `version`，放到 `exe` 旁边的 `.new`（第 6.6 节“更新”第 1–3 步）。
/// `progress` 收到已下载的字节数和总字节数（服务器没给时是 `None`）。
pub async fn download(
    client: &UpdateClient,
    version: Version,
    exe: &Path,
    progress: impl FnMut(u64, Option<u64>),
) -> Result<Staged, UpdateError> {
    download_with(client, &verify::official_key()?, version, exe, progress).await
}

async fn download_with(
    client: &UpdateClient,
    key: &PublicKey,
    version: Version,
    exe: &Path,
    progress: impl FnMut(u64, Option<u64>),
) -> Result<Staged, UpdateError> {
    let expected = signed_hash(client, key, version).await?;
    let staged = install::staged_path(exe);
    let actual = client
        .download(
            version,
            &release::program_file(version),
            &staged,
            client::MAX_PROGRAM,
            progress,
        )
        .await?;
    if actual != expected {
        let _ = std::fs::remove_file(&staged);
        return Err(UpdateError::Verify(
            "下载的程序和签过名的哈希对不上".to_owned(),
        ));
    }
    Ok(Staged {
        exe: exe.to_path_buf(),
        hash: expected,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::verify::tests::TestKey;
    use sha2::{Digest, Sha256};
    use wiremock::matchers::path;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const V041: Version = Version {
        major: 0,
        minor: 4,
        patch: 1,
    };
    const PROGRAM: &[u8] = b"MhyCarry 0.4.1";
    const BASE: &str = "/shulkx/mhy-carry/releases/download/v0.4.1/";
    const COMMENT: &str = "MhyCarry v0.4.1";

    fn parse(text: &str) -> Version {
        Version::parse(text).unwrap()
    }

    fn hex(bytes: &[u8]) -> String {
        Sha256::digest(bytes)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }

    fn sums_for(program: &[u8]) -> String {
        format!(
            "{}  MhyCarry_0.4.1_x64-portable.exe\n{}  MhyCarry_0.4.1_x64-setup.exe\n",
            hex(program),
            hex(b"setup")
        )
    }

    /// 假的 GitHub：最新发布是 v0.4.1，文件列表 `sums` 用 `key` 签名（`None` 表示没有签名文件），
    /// 实际提供下载的程序是 `program`。
    async fn release(key: Option<&TestKey>, sums: String, program: &[u8]) -> MockServer {
        let server = MockServer::start().await;
        Mock::given(path("/shulkx/mhy-carry/releases/latest"))
            .respond_with(ResponseTemplate::new(302).insert_header(
                "location",
                format!("{}/shulkx/mhy-carry/releases/tag/v0.4.1", server.uri()),
            ))
            .mount(&server)
            .await;
        let mut files = vec![
            ("SHA256SUMS", sums.clone().into_bytes()),
            ("MhyCarry_0.4.1_x64-portable.exe", program.to_vec()),
        ];
        if let Some(key) = key {
            files.push(("SHA256SUMS.minisig", key.sign(sums.as_bytes(), COMMENT)));
        }
        for (name, body) in files {
            Mock::given(path(format!("{BASE}{name}")))
                .respond_with(ResponseTemplate::new(200).set_body_bytes(body))
                .mount(&server)
                .await;
        }
        server
    }

    async fn requested_paths(server: &MockServer) -> Vec<String> {
        server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .map(|request| request.url.path().to_owned())
            .collect()
    }

    /// 临时文件夹里的“正在运行的程序”。
    fn program() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("MhyCarry.exe");
        std::fs::write(&exe, "MhyCarry 0.4.0").unwrap();
        (dir, exe)
    }

    #[tokio::test]
    async fn newer_signed_releases_can_be_installed() {
        let key = TestKey::new();
        let server = release(Some(&key), sums_for(PROGRAM), PROGRAM).await;
        let client = UpdateClient::for_tests(&server.uri());
        let release = check_with(&client, &key.public(), parse("0.4.0"))
            .await
            .unwrap();
        assert_eq!(
            release,
            Some(Release {
                version: V041,
                installable: true
            })
        );
        // 检查时只下载文件列表和签名，不下载程序。
        let paths = requested_paths(&server).await;
        assert!(
            !paths.iter().any(|path| path.ends_with(".exe")),
            "{paths:?}"
        );
    }

    #[tokio::test]
    async fn unsigned_or_wrongly_signed_releases_can_only_be_downloaded_by_hand() {
        let key = TestKey::new();
        for server in [
            release(None, sums_for(PROGRAM), PROGRAM).await,
            release(Some(&TestKey::new()), sums_for(PROGRAM), PROGRAM).await,
        ] {
            let client = UpdateClient::for_tests(&server.uri());
            let release = check_with(&client, &key.public(), parse("0.4.0"))
                .await
                .unwrap();
            assert_eq!(
                release,
                Some(Release {
                    version: V041,
                    installable: false
                })
            );
        }
    }

    #[tokio::test]
    async fn no_newer_version_means_nothing_else_is_downloaded() {
        let key = TestKey::new();
        let server = release(Some(&key), sums_for(PROGRAM), PROGRAM).await;
        let client = UpdateClient::for_tests(&server.uri());
        for current in ["0.4.1", "0.5.0"] {
            assert_eq!(
                check_with(&client, &key.public(), parse(current))
                    .await
                    .unwrap(),
                None
            );
        }
        let paths = requested_paths(&server).await;
        assert!(
            !paths.iter().any(|path| path.contains("/download/")),
            "{paths:?}"
        );
    }

    #[tokio::test]
    async fn network_errors_while_checking_the_signature_are_errors() {
        let server = MockServer::start().await;
        Mock::given(path("/shulkx/mhy-carry/releases/latest"))
            .respond_with(ResponseTemplate::new(302).insert_header(
                "location",
                format!("{}/shulkx/mhy-carry/releases/tag/v0.4.1", server.uri()),
            ))
            .mount(&server)
            .await;
        Mock::given(path(format!("{BASE}SHA256SUMS")))
            .respond_with(ResponseTemplate::new(503))
            .mount(&server)
            .await;
        let client = UpdateClient::for_tests(&server.uri());
        let result = check_with(&client, &TestKey::new().public(), parse("0.4.0")).await;
        assert!(
            matches!(result, Err(UpdateError::Status(503))),
            "{result:?}"
        );
    }

    #[tokio::test]
    async fn signed_programs_replace_the_running_one() {
        let key = TestKey::new();
        let server = release(Some(&key), sums_for(PROGRAM), PROGRAM).await;
        let (_dir, exe) = program();
        let client = UpdateClient::for_tests(&server.uri());
        let staged = download_with(&client, &key.public(), V041, &exe, |_, _| {})
            .await
            .unwrap();
        assert_eq!(std::fs::read(install::staged_path(&exe)).unwrap(), PROGRAM);
        staged.install().unwrap();
        assert_eq!(std::fs::read(&exe).unwrap(), PROGRAM);
        assert_eq!(
            std::fs::read(install::old_path(&exe)).unwrap(),
            b"MhyCarry 0.4.0"
        );
    }

    #[tokio::test]
    async fn files_changed_before_the_swap_are_not_installed() {
        let key = TestKey::new();
        let server = release(Some(&key), sums_for(PROGRAM), PROGRAM).await;
        let (_dir, exe) = program();
        let client = UpdateClient::for_tests(&server.uri());
        let staged = download_with(&client, &key.public(), V041, &exe, |_, _| {})
            .await
            .unwrap();
        std::fs::write(install::staged_path(&exe), "被改过").unwrap();
        let result = staged.install();
        assert!(matches!(result, Err(UpdateError::Verify(_))), "{result:?}");
        assert_eq!(std::fs::read(&exe).unwrap(), b"MhyCarry 0.4.0");
        assert!(!install::old_path(&exe).exists());
    }

    #[tokio::test]
    async fn programs_that_differ_from_the_signed_hash_are_deleted() {
        let key = TestKey::new();
        let server = release(Some(&key), sums_for(PROGRAM), b"something else").await;
        let (_dir, exe) = program();
        let client = UpdateClient::for_tests(&server.uri());
        let result = download_with(&client, &key.public(), V041, &exe, |_, _| {}).await;
        assert!(matches!(result, Err(UpdateError::Verify(_))), "{result:?}");
        assert!(!install::staged_path(&exe).exists());
    }

    #[tokio::test]
    async fn other_keys_stop_before_the_program_is_downloaded() {
        let server = release(Some(&TestKey::new()), sums_for(PROGRAM), PROGRAM).await;
        let (_dir, exe) = program();
        let client = UpdateClient::for_tests(&server.uri());
        let result = download_with(&client, &TestKey::new().public(), V041, &exe, |_, _| {}).await;
        assert!(matches!(result, Err(UpdateError::Verify(_))), "{result:?}");
        assert!(!install::staged_path(&exe).exists());
        let paths = requested_paths(&server).await;
        assert!(
            !paths.iter().any(|path| path.ends_with(".exe")),
            "{paths:?}"
        );
    }
}
