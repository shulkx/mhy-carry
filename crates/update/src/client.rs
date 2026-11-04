//! 更新客户端（设计文档第 6.6、9 节）：不带任何凭证和 Cookie，只能访问 GitHub 的两个域名，
//! 只允许 HTTPS，跳转也只能在这两个域名之间。
//! 查版本走网页的“最新发布”链接，不走 GitHub API：API 不登录时每个 IP 每小时只能请求 60 次，
//! 共用出口 IP 的网络很容易用光。

use std::path::Path;
use std::time::Duration;

use reqwest::header::LOCATION;
use reqwest::redirect::{Attempt, Policy};
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;
use url::Url;

use crate::error::UpdateError;
use crate::release::{self, REPO};
use crate::version::Version;

/// 允许访问的域名。下载会从 github.com 跳转到 release-assets.githubusercontent.com（2026-11-04 实测）。
pub const HOSTS: [&str; 2] = ["github.com", "release-assets.githubusercontent.com"];
/// 只写程序名，不带版本和其他标识。
const USER_AGENT: &str = "MhyCarry";
/// 查版本、下载文件列表和签名，整个请求最多这么久。
const TIMEOUT: Duration = Duration::from_secs(10);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// 下载程序时这么久没收到数据就算失败。
const READ_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_REDIRECTS: usize = 5;
/// `SHA256SUMS` 和签名的大小上限。
pub(crate) const MAX_SMALL: u64 = 64 * 1024;
/// 程序文件的大小上限。现在的 exe 约 16 MB。
pub(crate) const MAX_PROGRAM: u64 = 64 * 1024 * 1024;

pub struct UpdateClient {
    /// 下载用：跳转只能在白名单之间。
    http: reqwest::Client,
    /// 查最新版本用：不跟随跳转，只读跳转地址。
    probe: reqwest::Client,
    site: Url,
    /// 测试时允许访问本地假服务器（http://127.0.0.1）。
    allow_local: bool,
}

impl UpdateClient {
    pub fn new() -> Result<Self, UpdateError> {
        Self::build("https://github.com/", false)
    }

    /// 指向本地假服务器。
    #[cfg(test)]
    pub(crate) fn for_tests(base: &str) -> Self {
        Self::build(base, true).expect("构建测试客户端")
    }

    fn build(site: &str, allow_local: bool) -> Result<Self, UpdateError> {
        let policy = Policy::custom(move |attempt: Attempt| {
            if attempt.previous().len() > MAX_REDIRECTS {
                attempt.error("跳转次数太多")
            } else if allowed(attempt.url(), allow_local) {
                attempt.follow()
            } else {
                attempt.error("跳转到了白名单以外的地址")
            }
        });
        let http = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .redirect(policy)
            .referer(false)
            .connect_timeout(CONNECT_TIMEOUT)
            .read_timeout(READ_TIMEOUT)
            .https_only(!allow_local)
            .build()
            .map_err(UpdateError::network)?;
        let probe = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .redirect(Policy::none())
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(TIMEOUT)
            .https_only(!allow_local)
            .build()
            .map_err(UpdateError::network)?;
        Ok(Self {
            http,
            probe,
            site: Url::parse(site).map_err(|error| UpdateError::Format(error.to_string()))?,
            allow_local,
        })
    }

    /// 查最新的正式版本：请求“最新发布”链接，不跟随跳转，从跳转地址里读出版本号。
    pub(crate) async fn latest(&self) -> Result<Version, UpdateError> {
        let url = self.join(&format!("{REPO}/releases/latest"))?;
        self.ensure_allowed(&url)?;
        let response = self
            .probe
            .get(url)
            .send()
            .await
            .map_err(UpdateError::network)?;
        let status = response.status();
        if !status.is_redirection() {
            return Err(if status.is_success() {
                UpdateError::Format("没有跳转到最新发布".to_owned())
            } else {
                UpdateError::Status(status.as_u16())
            });
        }
        let location = response
            .headers()
            .get(LOCATION)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default();
        release::version_from_location(&self.site, location)
            .ok_or_else(|| UpdateError::Format("最新发布的跳转地址不对".to_owned()))
    }

    /// 下载 `version` 的一个小文件（文件列表、签名），整个读进内存，超过 `limit` 就停。
    pub(crate) async fn fetch(
        &self,
        version: Version,
        file: &str,
        limit: u64,
    ) -> Result<Vec<u8>, UpdateError> {
        let mut response = self.send(self.asset(version, file)?, Some(TIMEOUT)).await?;
        if response
            .content_length()
            .is_some_and(|length| length > limit)
        {
            return Err(UpdateError::TooLarge(limit));
        }
        let mut body = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(UpdateError::network)? {
            if (body.len() + chunk.len()) as u64 > limit {
                return Err(UpdateError::TooLarge(limit));
            }
            body.extend_from_slice(&chunk);
        }
        Ok(body)
    }

    /// 把 `version` 的 `file` 下载到 `path`，边下载边算 SHA256，返回小写十六进制的哈希。
    /// `progress` 收到已下载的字节数和总字节数。失败时删掉没下完的文件。
    pub(crate) async fn download(
        &self,
        version: Version,
        file: &str,
        path: &Path,
        limit: u64,
        progress: impl FnMut(u64, Option<u64>),
    ) -> Result<String, UpdateError> {
        let result = self.download_to(version, file, path, limit, progress).await;
        if result.is_err() {
            let _ = tokio::fs::remove_file(path).await;
        }
        result
    }

    async fn download_to(
        &self,
        version: Version,
        file: &str,
        path: &Path,
        limit: u64,
        mut progress: impl FnMut(u64, Option<u64>),
    ) -> Result<String, UpdateError> {
        let mut response = self.send(self.asset(version, file)?, None).await?;
        let total = response.content_length();
        if total.is_some_and(|total| total > limit) {
            return Err(UpdateError::TooLarge(limit));
        }
        let mut output = tokio::fs::File::create(path).await?;
        let mut hasher = Sha256::new();
        let mut done = 0;
        progress(done, total);
        while let Some(chunk) = response.chunk().await.map_err(UpdateError::network)? {
            done += chunk.len() as u64;
            if done > limit {
                return Err(UpdateError::TooLarge(limit));
            }
            hasher.update(&chunk);
            output.write_all(&chunk).await?;
            progress(done, total);
        }
        output.sync_all().await?;
        Ok(hex(&hasher.finalize()))
    }

    /// 发布里某个文件的下载地址，由程序按版本号拼出。
    fn asset(&self, version: Version, file: &str) -> Result<Url, UpdateError> {
        self.join(&format!("{REPO}/releases/download/v{version}/{file}"))
    }

    fn join(&self, path: &str) -> Result<Url, UpdateError> {
        self.site
            .join(path)
            .map_err(|error| UpdateError::Format(error.to_string()))
    }

    fn ensure_allowed(&self, url: &Url) -> Result<(), UpdateError> {
        if allowed(url, self.allow_local) {
            Ok(())
        } else {
            Err(UpdateError::Blocked(format!(
                "不允许访问 {}",
                url.host_str().unwrap_or_default()
            )))
        }
    }

    async fn send(
        &self,
        url: Url,
        timeout: Option<Duration>,
    ) -> Result<reqwest::Response, UpdateError> {
        self.ensure_allowed(&url)?;
        let mut request = self.http.get(url);
        if let Some(timeout) = timeout {
            request = request.timeout(timeout);
        }
        let response = request.send().await.map_err(UpdateError::network)?;
        if !response.status().is_success() {
            return Err(UpdateError::Status(response.status().as_u16()));
        }
        Ok(response)
    }
}

/// 只允许 HTTPS 和白名单里的域名，地址里不能带用户名和密码。测试时另外允许本地假服务器。
fn allowed(url: &Url, allow_local: bool) -> bool {
    if !url.username().is_empty() || url.password().is_some() {
        return false;
    }
    let host = url.host_str().unwrap_or_default();
    match url.scheme() {
        "https" => HOSTS.contains(&host),
        "http" => allow_local && host == "127.0.0.1",
        _ => false,
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const V041: Version = Version {
        major: 0,
        minor: 4,
        patch: 1,
    };
    const LATEST_PATH: &str = "/shulkx/mhy-carry/releases/latest";
    const SUMS_PATH: &str = "/shulkx/mhy-carry/releases/download/v0.4.1/SHA256SUMS";

    #[test]
    fn only_github_hosts_over_https_are_allowed() {
        for url in [
            "https://github.com/shulkx/mhy-carry/releases/latest",
            "https://github.com/shulkx/mhy-carry/releases/download/v0.4.1/SHA256SUMS",
            "https://release-assets.githubusercontent.com/github-production-release-asset/1",
        ] {
            assert!(allowed(&Url::parse(url).unwrap(), false), "{url}");
        }
        for url in [
            "http://github.com/x",
            "https://api.github.com/repos/shulkx/mhy-carry/releases/latest",
            "https://evil.cn/x",
            "https://github.com.evil.cn/x",
            "https://objects.githubusercontent.com/x",
            "https://user:pw@github.com/x",
            "http://127.0.0.1:8080/x",
            "ftp://github.com/x",
        ] {
            assert!(!allowed(&Url::parse(url).unwrap(), false), "{url}");
        }
        assert!(allowed(
            &Url::parse("http://127.0.0.1:8080/x").unwrap(),
            true
        ));
    }

    #[tokio::test]
    async fn requests_outside_the_whitelist_are_not_sent() {
        let client = UpdateClient::new().unwrap();
        for url in ["https://evil.cn/x", "http://github.com/x"] {
            let result = client.send(Url::parse(url).unwrap(), None).await;
            assert!(
                matches!(result, Err(UpdateError::Blocked(_))),
                "{url}: {result:?}"
            );
        }
    }

    #[tokio::test]
    async fn latest_version_comes_from_the_redirect_without_credentials() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(LATEST_PATH))
            .and(header("user-agent", "MhyCarry"))
            .respond_with(ResponseTemplate::new(302).insert_header(
                "location",
                format!("{}/shulkx/mhy-carry/releases/tag/v0.4.1", server.uri()),
            ))
            .mount(&server)
            .await;
        // 不跟随跳转：发布页面本身不会被请求。
        Mock::given(path("/shulkx/mhy-carry/releases/tag/v0.4.1"))
            .respond_with(ResponseTemplate::new(200))
            .expect(0)
            .mount(&server)
            .await;
        let version = UpdateClient::for_tests(&server.uri())
            .latest()
            .await
            .unwrap();
        assert_eq!(version, V041);
        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests.len(), 1);
        for name in ["cookie", "authorization", "referer"] {
            assert!(!requests[0].headers.contains_key(name), "{name}");
        }
    }

    #[tokio::test]
    async fn latest_without_a_proper_redirect_is_an_error() {
        let server = MockServer::start().await;
        let client = UpdateClient::for_tests(&server.uri());
        // 没有这个页面。
        let result = client.latest().await;
        assert!(
            matches!(result, Err(UpdateError::Status(404))),
            "{result:?}"
        );

        Mock::given(path(LATEST_PATH))
            .respond_with(ResponseTemplate::new(302).insert_header(
                "location",
                "https://evil.cn/shulkx/mhy-carry/releases/tag/v0.4.1",
            ))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        let result = client.latest().await;
        assert!(matches!(result, Err(UpdateError::Format(_))), "{result:?}");

        Mock::given(path(LATEST_PATH))
            .respond_with(ResponseTemplate::new(200).set_body_string("<html></html>"))
            .mount(&server)
            .await;
        let result = client.latest().await;
        assert!(matches!(result, Err(UpdateError::Format(_))), "{result:?}");
    }

    #[tokio::test]
    async fn redirects_inside_the_whitelist_are_followed() {
        let server = MockServer::start().await;
        Mock::given(path(SUMS_PATH))
            .respond_with(
                ResponseTemplate::new(302)
                    .insert_header("location", format!("{}/assets/1", server.uri())),
            )
            .mount(&server)
            .await;
        Mock::given(path("/assets/1"))
            .respond_with(ResponseTemplate::new(200).set_body_string("sums"))
            .mount(&server)
            .await;
        let body = UpdateClient::for_tests(&server.uri())
            .fetch(V041, "SHA256SUMS", MAX_SMALL)
            .await
            .unwrap();
        assert_eq!(body, b"sums");
    }

    #[tokio::test]
    async fn redirects_out_of_the_whitelist_are_blocked() {
        let server = MockServer::start().await;
        Mock::given(path(SUMS_PATH))
            .respond_with(
                ResponseTemplate::new(302).insert_header("location", "https://evil.cn/sums"),
            )
            .mount(&server)
            .await;
        let result = UpdateClient::for_tests(&server.uri())
            .fetch(V041, "SHA256SUMS", MAX_SMALL)
            .await;
        assert!(matches!(result, Err(UpdateError::Blocked(_))), "{result:?}");
    }

    #[tokio::test]
    async fn endless_redirects_are_blocked() {
        let server = MockServer::start().await;
        Mock::given(path(SUMS_PATH))
            .respond_with(
                ResponseTemplate::new(302)
                    .insert_header("location", format!("{}{SUMS_PATH}", server.uri())),
            )
            .mount(&server)
            .await;
        let result = UpdateClient::for_tests(&server.uri())
            .fetch(V041, "SHA256SUMS", MAX_SMALL)
            .await;
        assert!(matches!(result, Err(UpdateError::Blocked(_))), "{result:?}");
    }

    #[tokio::test]
    async fn oversized_files_are_refused() {
        let server = MockServer::start().await;
        Mock::given(path(SUMS_PATH))
            .respond_with(ResponseTemplate::new(200).set_body_string("x".repeat(100)))
            .mount(&server)
            .await;
        let result = UpdateClient::for_tests(&server.uri())
            .fetch(V041, "SHA256SUMS", 10)
            .await;
        assert!(
            matches!(result, Err(UpdateError::TooLarge(10))),
            "{result:?}"
        );
    }

    #[tokio::test]
    async fn downloads_report_progress_and_the_hash() {
        let server = MockServer::start().await;
        let file = "MhyCarry_0.4.1_x64-portable.exe";
        Mock::given(path(format!(
            "/shulkx/mhy-carry/releases/download/v0.4.1/{file}"
        )))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(b"new program".to_vec()))
        .mount(&server)
        .await;
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("MhyCarry.exe.new");
        let mut last = None;
        let hash = UpdateClient::for_tests(&server.uri())
            .download(V041, file, &target, MAX_PROGRAM, |done, total| {
                last = Some((done, total))
            })
            .await
            .unwrap();
        assert_eq!(hash, hex(&Sha256::digest(b"new program")));
        assert_eq!(std::fs::read(&target).unwrap(), b"new program");
        assert_eq!(last, Some((11, Some(11))));
    }

    #[tokio::test]
    async fn failed_downloads_leave_no_file() {
        let server = MockServer::start().await;
        let file = "MhyCarry_0.4.1_x64-portable.exe";
        Mock::given(path(format!(
            "/shulkx/mhy-carry/releases/download/v0.4.1/{file}"
        )))
        .respond_with(ResponseTemplate::new(200).set_body_string("x".repeat(100)))
        .mount(&server)
        .await;
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("MhyCarry.exe.new");
        let result = UpdateClient::for_tests(&server.uri())
            .download(V041, file, &target, 10, |_, _| {})
            .await;
        assert!(
            matches!(result, Err(UpdateError::TooLarge(10))),
            "{result:?}"
        );
        assert!(!target.exists());
    }
}
