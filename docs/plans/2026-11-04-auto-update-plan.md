# 检查更新和自动更新 实施计划

> **给执行者：** 在当前会话里按顺序执行（executing-plans），不要每个任务开一个子代理。步骤用复选框（`- [ ]`）跟踪。

**目标：** 打开程序时查 GitHub 上有没有新版本，有就在页面最上面提示。点“更新并重启”后，程序下载新版本、核对签名，直接替换自己的 exe 然后重启；安装版和免安装版都这样，不运行安装包。发布 v0.4.0（设计文档第 6.6 节、第 13 节 M7，2026-11-04 和用户确认）。

**做法：**
- `core`：设置里加 `update.check_on_start`，默认开。
- 新包 `update`（`mhy-carry-update`，不依赖其他单元）：
  - 版本号；
  - 从“最新发布”链接的跳转地址里读出版本号；
  - 更新客户端：只能访问 GitHub 的两个域名；
  - 签名核对，用 `minisign-verify`；
  - 下载到 `.new`，换掉正在运行的 exe。
- `platform`：安装版在注册表里的版本号；用默认浏览器打开发布页面。
- 应用：
  - 状态快照里加 `update` 和 `version`；
  - 四个新命令：`check_update`、`install_update`、`open_release_page`、`set_check_updates`；
  - 页面最上面的提示条，设置里的版本、“检查更新”按钮和开关。
- `release.yml`：新增 `sign` job，生成 `SHA256SUMS.minisig`。

**依据：** 设计文档 [docs/design/2026-10-11-mhy-carry-design.md](../design/2026-10-11-mhy-carry-design.md) 第 2、4、6.3、6.6、8、9、11、12、13 节。

**新增依赖：**
- `minisign-verify` 0.3（MIT，没有其他依赖）；
- `sha2` 0.10（已经在 `Cargo.lock` 里）；
- 只在测试里用：`minisign` 0.10（MIT），用来生成测试用的密钥和签名。

**新增域名：** `github.com`、`release-assets.githubusercontent.com`，只给更新客户端用，用户已同意。原来还有 `api.github.com`，执行中改掉了，见执行记录。

---

## 约定

和以前的计划一样：
- 命令在仓库根目录执行；
- 每个任务提交一次，提交前 fmt、clippy、测试都要通过，带上 `Cargo.lock`；
- 新文件给出全文（`<!-- file: 路径 -->` 后面的代码块），改动给出要替换的那一段。

**签名密钥由用户本人生成**（任务 9 之前要做好）：
- 私钥和密码只在用户本机和仓库的 Actions secrets 里：不读，不贴进聊天，不提交。
- 只读公钥文件 `update.key.pub`。

```powershell
New-Item -ItemType Directory -Force "$env:USERPROFILE\.mhycarry-signing" | Out-Null
cargo tauri signer generate -w "$env:USERPROFILE\.mhycarry-signing\update.key"
Get-Content -Raw "$env:USERPROFILE\.mhycarry-signing\update.key" | gh secret set UPDATE_SIGNING_KEY -R shulkx/mhy-carry
gh secret set UPDATE_SIGNING_PASSWORD -R shulkx/mhy-carry
```

---

### 任务 1：设置里的“打开程序时检查更新”（`core`）

**文件：** 修改 `crates/core/src/settings.rs`

- [ ] **步骤 1：** `Settings` 的 `scan` 字段后面加：

```rust
    /// 检查更新（设计文档第 6.6 节）。v0.4 之前的文件没有这一块，读的时候用默认值。
    #[serde(default)]
    pub update: UpdateSettings,
```

`TargetKind` 的定义前面加：

```rust
/// 检查更新的设置（设计文档第 6.6 节）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct UpdateSettings {
    /// 打开程序时查一次新版本。默认开。
    pub check_on_start: bool,
}

impl Default for UpdateSettings {
    fn default() -> Self {
        Self {
            check_on_start: true,
        }
    }
}
```

`load_or_init` 里新建设置时，`scan: ScanSettings::default(),` 后面加 `update: UpdateSettings::default(),`。

- [ ] **步骤 2：** 测试模块里 `files_from_m1_get_default_scan_settings` 后面加：

```rust
    #[test]
    fn files_before_v04_check_for_updates() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        fs::write(
            &path,
            r#"{"format":1,"device_id":"0b8f4a1e-1111-4222-8333-944445555666","scan":{"default_target":"live"}}"#,
        )
        .unwrap();
        let settings = Settings::load_or_init(&path).unwrap();
        assert!(settings.update.check_on_start);
        assert_eq!(settings.scan.default_target, TargetKind::Live);
    }

    #[test]
    fn update_settings_are_saved() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        let mut settings = Settings::load_or_init(&path).unwrap();
        assert!(settings.update.check_on_start);
        settings.update.check_on_start = false;
        settings.save(&path).unwrap();
        assert!(!Settings::load_or_init(&path).unwrap().update.check_on_start);
    }
```

- [ ] **步骤 3：运行** `cargo test -p mhy-carry-core settings::`，预期全部通过。
- [ ] **步骤 4：提交** `core: 设置里加打开程序时检查更新`

### 任务 2：`update` 包：版本号、最新发布、错误

（`release.rs` 和任务 3、5 的代码是按执行记录里“查版本改走网页跳转”改过之后的版本；`lib.rs` 在任务 2、3 里先只写当时用得到的部分。）

**文件：**
- 修改 `Cargo.toml`（workspace）；
- 新建 `crates/update/Cargo.toml`、`src/lib.rs`、`src/version.rs`、`src/release.rs`、`src/error.rs`。

- [ ] **步骤 1：** workspace 的 `[workspace.dependencies]`：
  - `mhy-carry-live = …` 后面加 `mhy-carry-update = { path = "crates/update" }`；
  - `jiff = …` 后面加：

```toml
# 只在测试里用：生成测试用的密钥和签名（设计文档第 6.6 节）。
minisign = "0.10"
# 核对自动更新的签名。没有其他依赖。
minisign-verify = "0.3"
```

  - `serde_json = "1"` 后面加 `sha2 = "0.10"`。

<!-- file: crates/update/Cargo.toml -->
```toml
[package]
name = "mhy-carry-update"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
license.workspace = true
repository.workspace = true
publish.workspace = true

[dependencies]
minisign-verify.workspace = true
reqwest.workspace = true
sha2.workspace = true
thiserror.workspace = true
tokio = { workspace = true, features = ["fs", "io-util"] }
url.workspace = true

[dev-dependencies]
minisign.workspace = true
tempfile.workspace = true
wiremock.workspace = true
```

- [ ] **步骤 2：版本号**

<!-- file: crates/update/src/version.rs -->
```rust
//! 版本号：只认 `主.次.修订` 三段数字（设计文档第 6.6 节“检查”）。

use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Version {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
}

impl Version {
    /// 解析 `0.4.1` 这样的三段数字。前面带 `v`、预发布版本（`0.4.1-beta`）、多余的前导零都不认。
    pub fn parse(text: &str) -> Option<Self> {
        let mut parts = text.split('.').map(|part| {
            let digits = !part.is_empty()
                && part.len() <= 9
                && part.bytes().all(|byte| byte.is_ascii_digit());
            let leading_zero = part.len() > 1 && part.starts_with('0');
            if digits && !leading_zero {
                part.parse().ok()
            } else {
                None
            }
        });
        let version = Self {
            major: parts.next()??,
            minor: parts.next()??,
            patch: parts.next()??,
        };
        parts.next().is_none().then_some(version)
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_three_numbers() {
        assert_eq!(
            Version::parse("0.4.1"),
            Some(Version {
                major: 0,
                minor: 4,
                patch: 1
            })
        );
        assert_eq!(Version::parse("10.0.12").unwrap().to_string(), "10.0.12");
    }

    #[test]
    fn rejects_other_formats() {
        for text in [
            "",
            "0.4",
            "0.4.1.2",
            "v0.4.1",
            "0.4.1-beta",
            "0.4.x",
            "01.4.1",
            "0.4.",
            " 0.4.1",
            "9999999999.0.0",
        ] {
            assert_eq!(Version::parse(text), None, "{text}");
        }
    }

    #[test]
    fn compares_by_number() {
        let parse = |text| Version::parse(text).unwrap();
        assert!(parse("0.10.0") > parse("0.9.9"));
        assert!(parse("1.0.0") > parse("0.99.99"));
        assert!(parse("0.4.1") > parse("0.4.0"));
        assert_eq!(parse("0.4.1"), parse("0.4.1"));
    }
}
```

- [ ] **步骤 3：最新发布**

<!-- file: crates/update/src/release.rs -->
```rust
//! GitHub 上最新的正式发布（设计文档第 6.6 节“检查”）。

use url::Url;

use crate::version::Version;

pub(crate) const REPO: &str = "shulkx/mhy-carry";
/// 发布里签过名的文件列表，和它的签名。
pub(crate) const SUMS: &str = "SHA256SUMS";
pub(crate) const SUMS_SIGNATURE: &str = "SHA256SUMS.minisig";

/// 免安装版 exe 的文件名，和 `release.yml` 里的一致。
pub(crate) fn program_file(version: Version) -> String {
    format!("MhyCarry_{version}_x64-portable.exe")
}

/// 发布页面的地址。不知道版本时是最新发布的页面。
pub fn release_page(version: Option<Version>) -> String {
    match version {
        Some(version) => format!("https://github.com/{REPO}/releases/tag/v{version}"),
        None => format!("https://github.com/{REPO}/releases/latest"),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Release {
    pub version: Version,
    /// 签名对得上，可以自动更新；否则只能到发布页面手动下载。
    pub installable: bool,
}

/// 从“最新发布”链接的跳转地址里读出版本号：`<site>/shulkx/mhy-carry/releases/tag/v<版本>`。
/// `site` 是请求的网站（正式版是 https://github.com/）。跳到别的网站、别的仓库，或者 tag 不是
/// `v<主>.<次>.<修订>` 的都不认。GitHub 的“最新发布”不含草稿和预发布。
pub(crate) fn version_from_location(site: &Url, location: &str) -> Option<Version> {
    let url = site.join(location).ok()?;
    if url.origin() != site.origin() {
        return None;
    }
    let tag = url
        .path()
        .strip_prefix(&format!("/{REPO}/releases/tag/"))?;
    Version::parse(tag.strip_prefix('v')?)
}

#[cfg(test)]
mod tests {
    use super::*;

    const V041: Version = Version {
        major: 0,
        minor: 4,
        patch: 1,
    };

    fn site() -> Url {
        Url::parse("https://github.com/").unwrap()
    }

    #[test]
    fn reads_the_version_from_the_redirect() {
        for location in [
            "https://github.com/shulkx/mhy-carry/releases/tag/v0.4.1",
            "/shulkx/mhy-carry/releases/tag/v0.4.1",
        ] {
            assert_eq!(version_from_location(&site(), location), Some(V041), "{location}");
        }
    }

    #[test]
    fn other_redirects_are_not_versions() {
        for location in [
            "https://evil.cn/shulkx/mhy-carry/releases/tag/v0.4.1",
            "http://github.com/shulkx/mhy-carry/releases/tag/v0.4.1",
            "https://github.com.evil.cn/shulkx/mhy-carry/releases/tag/v0.4.1",
            "https://github.com/other/mhy-carry/releases/tag/v0.4.1",
            "https://github.com/shulkx/mhy-carry/releases/tag/0.4.1",
            "https://github.com/shulkx/mhy-carry/releases/tag/v0.4.1-beta",
            "https://github.com/shulkx/mhy-carry/releases/tag/v0.4.1/x",
            "https://github.com/shulkx/mhy-carry/releases",
            "https://github.com/login",
            "",
        ] {
            assert_eq!(version_from_location(&site(), location), None, "{location}");
        }
    }

    #[test]
    fn release_pages() {
        assert_eq!(
            release_page(Some(V041)),
            "https://github.com/shulkx/mhy-carry/releases/tag/v0.4.1"
        );
        assert_eq!(
            release_page(None),
            "https://github.com/shulkx/mhy-carry/releases/latest"
        );
    }
}
```

- [ ] **步骤 4：错误**

<!-- file: crates/update/src/error.rs -->
```rust
//! 检查更新和自动更新出错的原因。都是给人看的中文句子。

#[derive(Debug, thiserror::Error)]
pub enum UpdateError {
    #[error("网络请求失败：{0}")]
    Network(reqwest::Error),
    #[error("GitHub 返回 HTTP {0}")]
    Status(u16),
    #[error("请求被拦截：{0}")]
    Blocked(String),
    #[error("GitHub 返回的内容格式不对：{0}")]
    Format(String),
    #[error("文件太大，超过了 {} 的上限", size(*.0))]
    TooLarge(u64),
    /// 只在更新失败时显示，界面前面已经写了“更新失败：”。
    #[error("{0}")]
    Verify(String),
    #[error("写入更新文件失败：{0}。可以到发布页面手动下载。")]
    Io(#[from] std::io::Error),
}

impl UpdateError {
    /// reqwest 的错误信息带完整网址，下载地址后面有一长串签名参数，先去掉。
    /// 被跳转规则拦下的请求算作被拦截。
    pub(crate) fn network(error: reqwest::Error) -> Self {
        if error.is_redirect() {
            return Self::Blocked(
                "下载地址跳到了 GitHub 以外的地方，或者跳转次数太多".to_owned(),
            );
        }
        Self::Network(error.without_url())
    }
}

fn size(bytes: u64) -> String {
    const MB: u64 = 1024 * 1024;
    if bytes >= MB {
        format!("{} MB", bytes / MB)
    } else {
        format!("{} KB", bytes.div_ceil(1024))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_are_readable() {
        assert!(UpdateError::TooLarge(64 * 1024 * 1024).to_string().contains("64 MB"));
        assert!(UpdateError::TooLarge(64 * 1024).to_string().contains("64 KB"));
    }
}
```

- [ ] **步骤 5：** `lib.rs` 先写成：

```rust
//! 检查更新和自动更新（设计文档第 6.6 节）：查 GitHub 上的最新发布，下载、核对新版本，
//! 换掉正在运行的程序。不依赖其他单元，也不依赖 Windows。
#![forbid(unsafe_code)]

mod error;
mod release;
mod version;

pub use error::UpdateError;
pub use release::{Release, release_page};
pub use version::Version;
```

- [ ] **步骤 6：运行** `cargo test -p mhy-carry-update`，预期全部通过；`cargo deny check` 通过。
- [ ] **步骤 7：提交** `update: 版本号和最新发布`

### 任务 3：更新客户端

**文件：** 新建 `crates/update/src/client.rs`；修改 `lib.rs`。

<!-- file: crates/update/src/client.rs -->
```rust
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
        if response.content_length().is_some_and(|length| length > limit) {
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
        assert!(allowed(&Url::parse("http://127.0.0.1:8080/x").unwrap(), true));
    }

    #[tokio::test]
    async fn requests_outside_the_whitelist_are_not_sent() {
        let client = UpdateClient::new().unwrap();
        for url in ["https://evil.cn/x", "http://github.com/x"] {
            let result = client.send(Url::parse(url).unwrap(), None).await;
            assert!(matches!(result, Err(UpdateError::Blocked(_))), "{url}: {result:?}");
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
        let version = UpdateClient::for_tests(&server.uri()).latest().await.unwrap();
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
        assert!(matches!(result, Err(UpdateError::Status(404))), "{result:?}");

        Mock::given(path(LATEST_PATH))
            .respond_with(
                ResponseTemplate::new(302)
                    .insert_header("location", "https://evil.cn/shulkx/mhy-carry/releases/tag/v0.4.1"),
            )
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
        assert!(matches!(result, Err(UpdateError::TooLarge(10))), "{result:?}");
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
        assert!(matches!(result, Err(UpdateError::TooLarge(10))), "{result:?}");
        assert!(!target.exists());
    }
}
```

- [ ] **步骤 1：** 写入 `client.rs`，`lib.rs` 加 `mod client;`、`pub use client::UpdateClient;`。`check` 要用到任务 5 的签名核对，到任务 5 再加。

- [ ] **步骤 2：运行** `cargo test -p mhy-carry-update client::`，预期全部通过。
- [ ] **步骤 3：提交** `update: 只能访问 GitHub 的更新客户端`

### 任务 4：换掉正在运行的程序

**文件：** 新建 `crates/update/src/install.rs`；`lib.rs` 加 `pub mod install;`。

<!-- file: crates/update/src/install.rs -->
```rust
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
```

- [ ] **步骤 1：运行** `cargo test -p mhy-carry-update install::`，预期全部通过。
- [ ] **步骤 2：提交** `update: 换掉正在运行的程序`

### 任务 5：签名核对和下载新版本

**文件：**
- 新建 `crates/update/update.pub`（用户生成的公钥）、`crates/update/src/verify.rs`；
- `lib.rs` 换成全文。

- [ ] **步骤 1：公钥。** 用户的公钥文件是 base64 包着的 minisign 公钥。解开后写进 `crates/update/update.pub`（两行：`untrusted comment: minisign public key: …` 和 `RW…`），LF 换行。

```bash
base64 -d "$USERPROFILE/.mhycarry-signing/update.key.pub" > crates/update/update.pub
```

<!-- file: crates/update/src/verify.rs -->
```rust
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
        let hash = expected_hash(&key.public(), sums.as_bytes(), &key.sign(sums.as_bytes()), PROGRAM);
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
        let result = expected_hash(&TestKey::new().public(), sums.as_bytes(), &signature, PROGRAM);
        assert!(matches!(result, Err(UpdateError::Verify(_))), "{result:?}");
    }

    #[test]
    fn an_old_signed_list_cannot_install_a_newer_version() {
        let key = TestKey::new();
        let old = sums().replace("0.4.1", "0.4.0");
        let result = expected_hash(&key.public(), old.as_bytes(), &key.sign(old.as_bytes()), PROGRAM);
        assert!(
            matches!(&result, Err(UpdateError::Verify(reason)) if reason.contains(PROGRAM)),
            "{result:?}"
        );
    }

    #[test]
    fn garbage_signatures_are_rejected() {
        let key = TestKey::new();
        for signature in [&b""[..], b"untrusted comment: x\nnot base64\n", &[0xff, 0xfe]] {
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
```

<!-- file: crates/update/src/lib.rs -->
```rust
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
    verify::expected_hash(key, &sums, &signature, &release::program_file(version))
}

/// 下载并核对 `version`，放到 `exe` 旁边的 `.new`，返回它的路径（第 6.6 节“更新”第 1–3 步）。
/// `progress` 收到已下载的字节数和总字节数（服务器没给时是 `None`）。
pub async fn download(
    client: &UpdateClient,
    version: Version,
    exe: &Path,
    progress: impl FnMut(u64, Option<u64>),
) -> Result<PathBuf, UpdateError> {
    download_with(client, &verify::official_key()?, version, exe, progress).await
}

async fn download_with(
    client: &UpdateClient,
    key: &PublicKey,
    version: Version,
    exe: &Path,
    progress: impl FnMut(u64, Option<u64>),
) -> Result<PathBuf, UpdateError> {
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
    Ok(staged)
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
            files.push(("SHA256SUMS.minisig", key.sign(sums.as_bytes())));
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
        assert!(!paths.iter().any(|path| path.ends_with(".exe")), "{paths:?}");
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
        assert!(!paths.iter().any(|path| path.contains("/download/")), "{paths:?}");
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
        assert!(matches!(result, Err(UpdateError::Status(503))), "{result:?}");
    }

    #[tokio::test]
    async fn signed_programs_are_downloaded_next_to_the_exe() {
        let key = TestKey::new();
        let server = release(Some(&key), sums_for(PROGRAM), PROGRAM).await;
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("MhyCarry.exe");
        let client = UpdateClient::for_tests(&server.uri());
        let staged = download_with(&client, &key.public(), V041, &exe, |_, _| {})
            .await
            .unwrap();
        assert_eq!(staged, install::staged_path(&exe));
        assert_eq!(std::fs::read(&staged).unwrap(), PROGRAM);
    }

    #[tokio::test]
    async fn programs_that_differ_from_the_signed_hash_are_deleted() {
        let key = TestKey::new();
        let server = release(Some(&key), sums_for(PROGRAM), b"something else").await;
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("MhyCarry.exe");
        let client = UpdateClient::for_tests(&server.uri());
        let result = download_with(&client, &key.public(), V041, &exe, |_, _| {}).await;
        assert!(matches!(result, Err(UpdateError::Verify(_))), "{result:?}");
        assert!(!install::staged_path(&exe).exists());
    }

    #[tokio::test]
    async fn other_keys_stop_before_the_program_is_downloaded() {
        let server = release(Some(&TestKey::new()), sums_for(PROGRAM), PROGRAM).await;
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("MhyCarry.exe");
        let client = UpdateClient::for_tests(&server.uri());
        let result = download_with(&client, &TestKey::new().public(), V041, &exe, |_, _| {}).await;
        assert!(matches!(result, Err(UpdateError::Verify(_))), "{result:?}");
        assert!(!install::staged_path(&exe).exists());
        let paths = requested_paths(&server).await;
        assert!(!paths.iter().any(|path| path.ends_with(".exe")), "{paths:?}");
    }
}
```

- [ ] **步骤 2：运行** `cargo test -p mhy-carry-update`，预期全部通过（`verifies_a_signature_made_in_ci` 默认忽略）。
- [ ] **步骤 3：提交** `update: 核对签名、下载新版本`（带上 `update.pub`）

### 任务 6：注册表里的版本号、打开发布页面（`platform`）

**文件：**
- 修改 workspace `Cargo.toml`：`windows` 的 features 加 `"Win32_System_Registry"`、`"Win32_UI_Shell"`；
- 新建 `crates/platform/src/install.rs`、`crates/platform/src/shell.rs`；
- `lib.rs` 加两个模块。

- [ ] **步骤 1：** `crates/platform/src/lib.rs` 的说明改成“和操作系统打交道的部分：数据目录、窗口枚举、截图、直播画面解码，以及自动更新用到的注册表和浏览器。除了数据目录都只有 Windows 实现。”，`pub mod console;` 前面加：

```rust
#[cfg(windows)]
pub mod install;
```

`pub mod paths;` 后面加：

```rust
#[cfg(windows)]
pub mod shell;
```

<!-- file: crates/platform/src/install.rs -->
```rust
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
    let status =
        unsafe { RegGetValueW(root, &key, &name, RRF_RT_REG_SZ, None, None, Some(&mut size)) };
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
    let end = buffer.iter().position(|&unit| unit == 0).unwrap_or(buffer.len());
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
        write_string(HKEY_CURRENT_USER, &key.0, "InstallLocation", r#""C:\Apps\MhyCarry""#)
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
```

<!-- file: crates/platform/src/shell.rs -->
```rust
//! 用默认浏览器打开网页（设计文档第 6.6 节“界面”）。只给 Rust 端用，地址由程序拼出。

use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
use windows::core::{HSTRING, PCWSTR, w};

/// 只打开 https 网址。
pub fn open_in_browser(url: &str) -> Result<(), String> {
    if !url.starts_with("https://") {
        return Err("只能打开 https 网址。".to_owned());
    }
    let result = unsafe {
        ShellExecuteW(
            None,
            w!("open"),
            &HSTRING::from(url),
            PCWSTR::null(),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        )
    };
    // 返回值大于 32 表示成功。
    let code = result.0 as isize;
    if code > 32 {
        Ok(())
    } else {
        Err(format!("打不开浏览器（错误码 {code}）。"))
    }
}
```

- [ ] **步骤 2：运行** `cargo test -p mhy-carry-platform install::`，预期全部通过；跑完后 `reg query HKCU\Software` 里没有 `MhyCarryTest-*`。
- [ ] **步骤 3：提交** `platform: 安装版的版本号和打开发布页面`

### 任务 7：应用的状态和命令

**文件：**
- 修改 `app/src-tauri/Cargo.toml`（加 `mhy-carry-update.workspace = true`）、`state.rs`、`model.rs`、`commands.rs`、`main.rs`、`build.rs`、`capabilities/main.json`；
- 新建 `app/src-tauri/src/updater.rs`。

- [ ] **步骤 1：`state.rs`**
  - `AppState` 的 `leftovers` 后面加：

```rust
    /// 检查更新和自动更新（设计文档第 6.6 节）。
    pub update: UpdateView,
    /// 当前版本，比如 0.4.0。
    pub version: String,
```

  - `SettingsView` 末尾加 `pub check_updates: bool,`；
  - `SettingsView` 前面加：

```rust
/// 检查更新和自动更新的状态（设计文档第 6.6 节）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum UpdateView {
    /// 没查过，或者打开程序时查失败了、没有新版本。
    Idle,
    Checking,
    /// 手动检查的结果：已是最新版本。
    Latest,
    /// 有新版本。`installable` 为 false 时只能到发布页面手动下载。
    Available { version: String, installable: bool },
    Downloading { version: String, percent: u8 },
    /// 下载核对完了，正在替换和重启。
    Installing { version: String },
    /// 已经换成新版本，但没能自动重启。
    NeedsRestart { version: String },
    /// 检查或更新失败。更新失败时 `version` 是要装的版本，可以重试。
    Failed {
        message: String,
        version: Option<String>,
    },
}

impl UpdateView {
    /// 正在下载或替换。这期间不能开始扫描和绑定。
    pub fn busy(&self) -> bool {
        matches!(self, Self::Downloading { .. } | Self::Installing { .. })
    }

    /// 现在可以开始更新的版本：有新版本并且能自动更新，或者上次更新失败了可以重试。
    pub fn installable(&self) -> Option<&str> {
        match self {
            Self::Available {
                version,
                installable: true,
            }
            | Self::Failed {
                version: Some(version),
                ..
            } => Some(version),
            _ => None,
        }
    }

    /// 发布页面对应的版本。
    pub fn version(&self) -> Option<&str> {
        match self {
            Self::Available { version, .. }
            | Self::Downloading { version, .. }
            | Self::Installing { version }
            | Self::NeedsRestart { version }
            | Self::Failed {
                version: Some(version),
                ..
            } => Some(version),
            _ => None,
        }
    }
}
```

  - 测试模块末尾加：

```rust
    #[test]
    fn only_signed_releases_and_failed_updates_can_be_installed() {
        let available = |installable| UpdateView::Available {
            version: "0.4.1".to_owned(),
            installable,
        };
        assert_eq!(available(true).installable(), Some("0.4.1"));
        assert_eq!(available(false).installable(), None);
        assert_eq!(available(false).version(), Some("0.4.1"));
        let retry = UpdateView::Failed {
            message: "网络请求失败".to_owned(),
            version: Some("0.4.1".to_owned()),
        };
        assert_eq!(retry.installable(), Some("0.4.1"));
        let check_failed = UpdateView::Failed {
            message: "网络请求失败".to_owned(),
            version: None,
        };
        assert_eq!(check_failed.installable(), None);
        assert_eq!(check_failed.version(), None);
        assert_eq!(UpdateView::Latest.installable(), None);
    }

    #[test]
    fn downloading_and_installing_are_busy() {
        let version = || "0.4.1".to_owned();
        assert!(UpdateView::Downloading { version: version(), percent: 5 }.busy());
        assert!(UpdateView::Installing { version: version() }.busy());
        assert!(!UpdateView::NeedsRestart { version: version() }.busy());
        assert!(!UpdateView::Checking.busy());
    }
```

- [ ] **步骤 2：`model.rs`**
  - `use crate::state::{…}` 加 `UpdateView`；
  - `Model` 的 `notice_at` 后面加 `pub update: UpdateView,`，`Shared::load` 里初始化成 `update: UpdateView::Idle,`；
  - `ensure_idle` 的绑定检查后面加：

```rust
        if self.update.busy() {
            return Err("正在更新，请稍等。".to_owned());
        }
```

  - `snapshot_of` 的 `live_room` 后面加 `check_updates: model.settings.update.check_on_start,`，`leftovers` 后面加：

```rust
        update: model.update.clone(),
        version: env!("CARGO_PKG_VERSION").to_owned(),
```

<!-- file: app/src-tauri/src/updater.rs -->
```rust
//! 检查更新和自动更新（设计文档第 6.6 节）。

use std::path::Path;
use std::time::Duration;

use mhy_carry_update::{Release, UpdateClient, UpdateError, Version, install};
use tauri::{AppHandle, Manager};

use crate::model::{Shared, update};
use crate::state::UpdateView;

/// 当前版本。版本号不是三段数字时（比如以后的预发布版本）不检查更新。
pub fn current_version() -> Option<Version> {
    Version::parse(env!("CARGO_PKG_VERSION"))
}

/// 打开程序时：删掉上次更新留下的旧版本；设置里开着就查一次新版本。
pub fn on_start(app: &AppHandle) {
    if let Ok(exe) = std::env::current_exe() {
        // 旧进程可能还没退出，删不掉时过一秒再试，最多 10 秒。
        std::thread::spawn(move || {
            for _ in 0..10 {
                if install::clean_up(&exe) {
                    break;
                }
                std::thread::sleep(Duration::from_secs(1));
            }
        });
    }
    if app.state::<Shared>().lock().settings.update.check_on_start {
        start_check(app, false);
    }
}

/// 开始检查更新。已经在检查、下载或替换时不再查。`manual` 为 false（打开程序时）时，
/// 查失败了、没有新版本都不提示。
pub fn start_check(app: &AppHandle, manual: bool) {
    let Some(current) = current_version() else {
        return;
    };
    let mut start = false;
    update(app, |model| {
        if !model.update.busy() && model.update != UpdateView::Checking {
            model.update = UpdateView::Checking;
            start = true;
        }
    });
    if start {
        tauri::async_runtime::spawn(check(app.clone(), current, manual));
    }
}

async fn check(app: AppHandle, current: Version, manual: bool) {
    let result = latest(current).await;
    update(&app, |model| {
        model.update = match result {
            Ok(Some(release)) => UpdateView::Available {
                version: release.version.to_string(),
                installable: release.installable,
            },
            Ok(None) if manual => UpdateView::Latest,
            Err(error) if manual => UpdateView::Failed {
                message: error.to_string(),
                version: None,
            },
            _ => UpdateView::Idle,
        };
    });
}

async fn latest(current: Version) -> Result<Option<Release>, UpdateError> {
    mhy_carry_update::check(&UpdateClient::new()?, current).await
}

/// 下载、核对、换上 `version`，然后重启。调用前状态已经设成下载中。
pub async fn install(app: AppHandle, version: Version) {
    let label = version.to_string();
    let failed = |message: String| UpdateView::Failed {
        message,
        version: Some(label.clone()),
    };
    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(error) => {
            let message = format!("找不到程序文件：{error}");
            return update(&app, |model| model.update = failed(message));
        }
    };
    let progress = |done: u64, total: Option<u64>| {
        let percent = total
            .filter(|&total| total > 0)
            .map_or(0, |total| (done.min(total) * 100 / total) as u8);
        update(&app, |model| {
            model.update = UpdateView::Downloading {
                version: label.clone(),
                percent,
            }
        });
    };
    if let Err(error) = download(version, &exe, progress).await {
        return update(&app, |model| model.update = failed(error.to_string()));
    }
    update(&app, |model| {
        model.update = UpdateView::Installing {
            version: label.clone(),
        }
    });
    if let Err(error) = install::swap(&exe) {
        let message = format!("换不上新版本：{error}。可以到发布页面手动下载。");
        return update(&app, |model| model.update = failed(message));
    }
    // 安装版：“设置 → 应用”里的版本号跟着变。改不了也不影响使用。
    if let Some(dir) = exe.parent() {
        let _ = mhy_carry_platform::install::record_version(dir, &label);
    }
    match std::process::Command::new(&exe).spawn() {
        Ok(_) => app.exit(0),
        Err(_) => update(&app, |model| {
            model.update = UpdateView::NeedsRestart {
                version: label.clone(),
            }
        }),
    }
}

async fn download(
    version: Version,
    exe: &Path,
    progress: impl FnMut(u64, Option<u64>),
) -> Result<(), UpdateError> {
    let client = UpdateClient::new()?;
    mhy_carry_update::download(&client, version, exe, progress).await?;
    Ok(())
}
```

- [ ] **步骤 3：`commands.rs`**
  - `use` 里加 `use mhy_carry_update::{Version, release_page};`，`use crate::state::{…}` 加 `UpdateView`，`use crate::{bind, scan};` 改成 `use crate::{bind, scan, updater};`；
  - `scan_start` 里 `if model.bind.in_progress() { … }` 后面加：

```rust
        if model.update.busy() {
            return Err("正在更新，不能开始扫描。".to_owned());
        }
```

  - 文件末尾加：

```rust
/// 手动检查更新（设计文档第 6.6 节）。
#[tauri::command]
pub fn check_update(app: AppHandle) {
    updater::start_check(&app, true);
}

/// 下载、核对并换上新版本，然后重启（设计文档第 6.6 节）。扫描中、绑定中不能更新。
#[tauri::command]
pub fn install_update(app: AppHandle) -> CommandResult {
    let mut result = Ok(());
    let mut target = None;
    update(&app, |model| {
        if model.scanning() {
            result = Err("扫描中不能更新，先停止扫描。".to_owned());
            return;
        }
        if model.bind.in_progress() {
            result = Err("绑定中不能更新。".to_owned());
            return;
        }
        match model.update.installable().and_then(Version::parse) {
            Some(version) => {
                model.update = UpdateView::Downloading {
                    version: version.to_string(),
                    percent: 0,
                };
                target = Some(version);
            }
            None => result = Err("没有可以自动安装的新版本。".to_owned()),
        }
    });
    if let Some(version) = target {
        tauri::async_runtime::spawn(updater::install(app, version));
    }
    result
}

/// 用默认浏览器打开发布页面（设计文档第 6.6 节）。地址由程序按版本号拼出。
#[tauri::command]
pub fn open_release_page(shared: State<'_, Shared>) -> CommandResult {
    let version = shared.lock().update.version().and_then(Version::parse);
    mhy_carry_platform::shell::open_in_browser(&release_page(version))
}

/// 打开或关闭“打开程序时检查更新”，记在设置里（设计文档第 6.6 节）。
#[tauri::command]
pub fn set_check_updates(app: AppHandle, on: bool) -> CommandResult {
    let shared = app.state::<Shared>();
    let mut result = Ok(());
    update(&app, |model| {
        let previous = model.settings.update.check_on_start;
        model.settings.update.check_on_start = on;
        if let Err(error) = model.settings.save(&shared.settings_path) {
            model.settings.update.check_on_start = previous;
            result = Err(error.to_string());
        }
    });
    result
}
```

- [ ] **步骤 4：`main.rs`**
  - `mod state;` 后面加 `mod updater;`；
  - `setup` 里 `app.manage(…)?;` 后面加 `updater::on_start(app.handle());`；
  - `generate_handler!` 末尾加 `commands::check_update, commands::install_update, commands::open_release_page, commands::set_check_updates,`；
  - 测试模块加：

```rust
    /// 发布流程按 tauri.conf.json 的版本号打 tag，检查更新用的是 Cargo 的版本号，两个必须一致，
    /// 而且是三段数字，否则不会检查更新（设计文档第 6.6 节）。
    #[test]
    fn version_is_checkable_and_matches_the_bundle() {
        let config: serde_json::Value =
            serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
        assert_eq!(config["version"], env!("CARGO_PKG_VERSION"));
        assert!(crate::updater::current_version().is_some());
    }
```

- [ ] **步骤 5：** `build.rs` 的命令列表末尾加 `"check_update", "install_update", "open_release_page", "set_check_updates",`；`capabilities/main.json` 末尾加 `"allow-check-update", "allow-install-update", "allow-open-release-page", "allow-set-check-updates"`。
- [ ] **步骤 6：运行** `cargo clippy -p mhy-carry --all-targets -- -D warnings` 和 `cargo test -p mhy-carry`，预期通过；`permissions/autogenerated/` 多出四个 `.toml`，一起提交。
- [ ] **步骤 7：** 和任务 8 一起提交。

### 任务 8：界面

**文件：** 修改 `app/ui/index.html`、`style.css`、`main.js`。

- [ ] **步骤 1：`index.html`**
  - `<main class="content">` 下面第一行加：

```html
  <div class="update-bar" id="update-bar" role="status" hidden>
    <span class="update-text" id="update-text"></span>
    <span class="update-actions" id="update-actions"></span>
  </div>
```

  - 设置的 `settings-grid` 末尾（“无二维码自动停止”那一行后面）加：

```html
      <label for="setting-check-updates">检查更新</label>
      <label class="check"><input id="setting-check-updates" type="checkbox"> 打开程序时检查</label>
      <span>版本</span>
      <span class="update-check"><span id="current-version"></span><button type="button" class="btn small" id="check-update">检查更新</button><span class="meta" id="update-result" aria-live="polite"></span></span>
```

- [ ] **步骤 2：`style.css`** 在 `details.settings` 前面加：

```css
.update-bar {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 8px 12px;
  padding: 8px 12px;
  border-radius: 8px;
  background: var(--accent-soft);
  font-size: 13px;
}
.update-bar.bad { background: var(--bad-soft); color: var(--bad); }
.update-text { flex: 1 1 auto; overflow-wrap: anywhere; }
.update-actions { display: flex; gap: 8px; }
```

`.settings-grid input { … }` 后面加：

```css
.settings-grid input[type="checkbox"] { width: auto; }
.settings-grid .check { display: flex; align-items: center; gap: 6px; }
.update-check { display: flex; flex-wrap: wrap; align-items: center; gap: 6px 10px; }
```

- [ ] **步骤 3：`main.js`**
  - `const MAX_NOTE_CHARS = 20;` 后面加：

```js
const UPDATING = ["downloading", "installing"];
```

  - `accountsLocked` 改成：

```js
// 扫描中、绑定中、更新中不能动账号（设计文档第 9 节、第 6.6 节）。
function accountsLocked() {
  return state.scan.kind !== "idle" || BUSY_BIND.includes(state.bind.kind) || UPDATING.includes(state.update.kind);
}
```

  - `renderScan` 里 `const ready = …` 改成：

```js
    const ready = active && active.credential !== "invalid" && !BUSY_BIND.includes(state.bind.kind)
      && !UPDATING.includes(state.update.kind);
```

  - `// ---- 设置和页脚 ----` 前面加：

```js
// ---- 更新（设计文档第 6.6 节） ----

function renderUpdate() {
  const update = state.update;
  const actions = $("update-actions");
  actions.replaceChildren();
  // 扫描中、绑定中不能更新。
  const busy = state.scan.kind !== "idle" || BUSY_BIND.includes(state.bind.kind);
  const page = () => button("发布页面", "btn small", () => call("open_release_page"));
  const install = (label, className) => {
    const node = button(label, className, () => call("install_update"));
    node.disabled = busy;
    if (busy) node.title = "扫描或绑定结束后才能更新";
    return node;
  };
  let text = null;
  if (update.kind === "available" && update.installable) {
    text = "有新版本 v" + update.version + "。";
    actions.append(install("更新并重启", "btn small primary"), page());
  } else if (update.kind === "available") {
    text = "有新版本 v" + update.version + "，请到发布页面下载。";
    actions.append(page());
  } else if (update.kind === "downloading") {
    text = "正在下载 v" + update.version + "… " + update.percent + "%";
  } else if (update.kind === "installing") {
    text = "正在安装 v" + update.version + "，马上重启…";
  } else if (update.kind === "needs_restart") {
    text = "已经换成 v" + update.version + "，请关掉程序重新打开。";
  } else if (update.kind === "failed" && update.version) {
    text = "更新失败：" + update.message;
    actions.append(install("重试", "btn small"), page());
  }
  const bar = $("update-bar");
  bar.hidden = text === null;
  bar.classList.toggle("bad", update.kind === "failed");
  $("update-text").textContent = text ?? "";

  // 设置里的版本和检查结果。
  $("current-version").textContent = "v" + state.version;
  let result = "";
  if (update.kind === "checking") result = "正在检查…";
  else if (update.kind === "latest") result = "已是最新版本";
  else if (update.kind === "available") result = "有新版本 v" + update.version;
  else if (update.kind === "failed" && !update.version) result = "检查失败：" + update.message;
  $("update-result").textContent = result;
  $("check-update").disabled = update.kind === "checking" || UPDATING.includes(update.kind);
  $("setting-check-updates").checked = state.settings.check_updates;
}

```

  - `render` 里 `renderSettings();` 后面加 `renderUpdate();`；
  - 事件部分 `$("setting-timeout").addEventListener(…)` 后面加：

```js
$("check-update").addEventListener("click", () => call("check_update"));
$("setting-check-updates").addEventListener("change", async () => {
  await call("set_check_updates", { on: $("setting-check-updates").checked });
  // 没改成时按快照退回原来的样子。
  renderUpdate();
});
```

- [ ] **步骤 4：检查** `node --check app/ui/main.js`。
- [ ] **步骤 5：远程调试检查**（临时数据目录、假账号，`m8check/` 里的脚本）：
  - 打开程序后设置里显示当前版本。启动时会真的去 GitHub 查一次：本地版本和最新发布一样时，结果是空的，提示条不出现。
  - 点“检查更新”显示“正在检查…”，然后是“已是最新版本”。
  - 用假快照依次显示几种状态，核对提示条的文字和按钮：能自动更新、只能手动下载、下载中、安装中、要手动重启、更新失败；再核对检查失败只显示在设置里。
  - 扫描中“更新并重启”是灰的，下载中“开始扫描”和“+ 添加账号”是灰的。
  - 后端：
    - 没有新版本时 `install_update` 返回“没有可以自动安装的新版本。”；
    - 关掉“打开程序时检查”后 `settings.json` 里是 `false`，重启后不再查。
  - 深色主题下提示条看得清。
- [ ] **步骤 6：提交** `app: 检查更新和自动更新`

### 任务 9：发布流程签名（`release.yml`、`ci.yml`）

**文件：** 修改 `.github/workflows/release.yml`、`ci.yml`。

- [ ] **步骤 1：`release.yml`**
  - `build` job 后面加：

```yaml
  # 给 SHA256SUMS 签名，自动更新只装签名对得上的文件（设计文档第 6.6 节）。只有这个 job
  # 拿得到签名私钥：它不编译任何代码，只用系统软件源里的 minisign。
  sign:
    needs: build
    runs-on: ubuntu-latest
    permissions:
      contents: read
    steps:
      - uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1 # v7.0.1
        with:
          persist-credentials: false
          sparse-checkout: crates/update/update.pub
          sparse-checkout-cone-mode: false
      - uses: actions/download-artifact@3e5f45b2cfb9172054b4087a40e8e0b5a5461e7c # v8.0.1
        with:
          name: dist
          path: dist
      - name: 核对 SHA256SUMS
        working-directory: dist
        run: sha256sum -c SHA256SUMS
      - name: 安装 minisign
        run: sudo apt-get update -qq && sudo apt-get install -y -qq minisign
      - name: 签名
        env:
          SIGNING_KEY: ${{ secrets.UPDATE_SIGNING_KEY }}
          SIGNING_PASSWORD: ${{ secrets.UPDATE_SIGNING_PASSWORD }}
        run: |
          if [ -z "$SIGNING_KEY" ] || [ -z "$SIGNING_PASSWORD" ]; then
            echo "仓库没有设置签名私钥（UPDATE_SIGNING_KEY、UPDATE_SIGNING_PASSWORD）" >&2
            exit 1
          fi
          umask 077
          # Tauri 生成的私钥文件是 minisign 格式外面套一层 base64。
          printf '%s' "$SIGNING_KEY" | tr -d '\r\n ' | base64 -d > "$RUNNER_TEMP/update.key"
          printf '%s\n' "$SIGNING_PASSWORD" | minisign -S -s "$RUNNER_TEMP/update.key" \
            -m dist/SHA256SUMS -x dist/SHA256SUMS.minisig -t "MhyCarry $GITHUB_REF_NAME"
          rm -f "$RUNNER_TEMP/update.key"
      - name: 用仓库里的公钥核对签名
        run: minisign -V -p crates/update/update.pub -m dist/SHA256SUMS -x dist/SHA256SUMS.minisig
      - uses: actions/upload-artifact@043fb46d1a93c77aae656e7c1c64a875d1fc6a0a # v7.0.1
        with:
          name: signature
          path: dist/SHA256SUMS.minisig
          if-no-files-found: error
```

  - `attest` 的 `needs: build` 改成 `needs: [build, sign]`；`attest` 和 `release` 两个 job 里，下载 `dist` 的那一步后面都加：

```yaml
      - uses: actions/download-artifact@3e5f45b2cfb9172054b4087a40e8e0b5a5461e7c # v8.0.1
        with:
          name: signature
          path: dist
```

  - 发布说明的下载列表末尾加一行 `- \`SHA256SUMS.minisig\`：自动更新用的签名，程序只装签名对得上的文件。`。
  - 文件开头的说明：手动运行时也会签名，用来在打 tag 之前试跑签名。

- [ ] **步骤 2：`ci.yml`** 的 `test-portable` 加 `-p mhy-carry-update`。
- [ ] **步骤 3：提交** `ci: 发布时给 SHA256SUMS 签名`

### 任务 10：文档

- [ ] **README：**
  - “使用”里“多个账号”后面加一节“更新”：

```markdown
### 更新

- 打开程序时会去 GitHub 查一次有没有新版本，有的话页面最上面会提示。点“更新并重启”，程序会下载新版本、核对签名，换掉自己再重新打开；安装版和免安装版都一样，不用再运行安装包。扫描中、绑定中不能更新。
- 程序只装签名对得上的文件。签名私钥只有发布流程拿得到，公钥编在程序里。
- 不想自动检查的话，在“设置”里关掉“打开程序时检查”，需要时再点“检查更新”。
- 换下来的旧版本会暂时留在程序旁边，叫 `<程序文件名>.old`，新版本打开后自动删掉。万一新版本打不开，把它改回原来的文件名就能用回旧版本。
- v0.3.1 和更早的版本没有这个功能，要手动下载一次新版本。
```

  - “核对下载的文件”末尾加一句：`SHA256SUMS.minisig` 是自动更新用的签名。
  - “声明”里“没有遥测，不连其他服务器。”前面加：`- 检查更新时会连接 GitHub（\`github.com\`、\`release-assets.githubusercontent.com\`），只发送程序名，不带任何标识；可以在设置里关掉。`
- [ ] **CLAUDE.md：**
  - 进度：加“2026-11-04 加了检查更新和自动更新（设计文档第 6.6 节，计划 docs/plans/2026-11-04-auto-update-plan.md），还没发布”。
  - “米哈游接口”的网络白名单那一条：直播客户端、更新客户端各有自己的白名单，改它们同样要改设计文档并告诉用户。
  - 安全红线：更新签名的私钥和密码只在用户本机和仓库的 Actions secrets 里，不读、不贴进聊天、不提交。
  - 常用命令加核对 CI 签名的联网测试。
- [ ] **提交** `docs: 检查更新和自动更新的使用说明`，推送，等 CI。

### 任务 11：试跑签名（需要用户已经设置好 secrets）

- [ ] 手动运行发布流程：`gh workflow run release.yml --ref main`。只构建和签名，不生成构建来源证明，也不发布。
- [ ] 等它跑完，`sign` job 里“用仓库里的公钥核对签名”通过。
- [ ] `gh run download <运行 ID> -n dist -n signature -D <临时文件夹>`，再运行：

```powershell
$env:MHY_CARRY_SIGNED_DIR = "<临时文件夹>"; cargo test -p mhy-carry-update -- --ignored --nocapture verifies_a_signature_made_in_ci
```

预期打印“签名正确”：系统软件源里的 minisign 生成的签名，程序里的 `minisign-verify` 能核对。

### 任务 12：独立审查

- [ ] 派一个子代理只读审查这批改动，重点：
  - 签名核对能不能被绕过：换文件、换签名、旧签名冒充新版本、跳转到别的域名；
  - 下载和替换的失败路径：有没有留下坏的 exe，回退是否可靠；
  - 扫描中、绑定中、更新中的互斥；
  - `release.yml` 里私钥会不会落到别的 job、日志或产物里；
  - `unsafe` 的注册表和 `ShellExecuteW` 调用；
  - 界面和设计第 6.3、6.6 节一致。
- [ ] 修完再继续。

### 任务 13：发布 v0.4.0 并实测（先问用户）

- [ ] 版本号改成 `0.4.0`（`Cargo.toml`、`tauri.conf.json`），跑 `cargo build` 更新 `Cargo.lock`，提交 `release: 0.4.0`。
- [ ] 用户同意后打 tag `v0.4.0` 并推送，等发布流程跑完；Release 里有 `SHA256SUMS.minisig`。
- [ ] 下载四个文件，逐个 `gh attestation verify`，再 `sha256sum -c SHA256SUMS`。
- [ ] **免安装版实测：**
  1. 把本地版本号临时改成 `0.3.9`（不提交），`cargo build --release -p mhy-carry`；
  2. 把 exe 复制到临时文件夹，用临时数据目录打开；
  3. 应该看到“有新版本 v0.4.0”，点“更新并重启”；
  4. 程序重启后是 v0.4.0：exe 的 SHA256 和 Release 里的免安装版一致，`.old` 几秒后被删掉。
- [ ] **安装版实测**（会改你装好的程序，先问用户）：
  1. 把同一个 0.3.9 的 exe 复制到 `%LOCALAPPDATA%\MhyCarry\mhy-carry.exe`，原来的先备份；
  2. 打开后更新；
  3. 重启后是 v0.4.0，注册表里的 `DisplayVersion` 变成 `0.4.0`。
- [ ] 设计文档状态行和第 14 节加实测结论，计划打勾，CLAUDE.md 更新进度，提交并推送。

---

## 执行记录

- **任务 3、5：查版本改走网页跳转**（2026-11-04 和用户确认）。
  - 原计划查 GitHub API `releases/latest`。任务 8 远程调试时，手动检查返回 403：不登录的 API 每个 IP 每小时只能请求 60 次，本机所在网络的出口 IP 已经用光了。用户的网络如果也共用出口 IP，会经常查不到新版本，而且打开程序时查失败不提示，用户不会知道。
  - 改成请求网页上的“最新发布”链接 `github.com/shulkx/mhy-carry/releases/latest`，不跟随跳转，从 302 的跳转地址里读版本号。实测约 0.6 秒，不受 API 次数限制。
  - 能不能自动更新，原来看发布里有没有那几个文件，现在改成下载 `SHA256SUMS` 和签名、真的核对一遍。没有签名文件时 GitHub 返回 404，算作只能手动下载。
  - 白名单去掉 `api.github.com`，只剩两个域名；`update` 包不再用 `serde`、`serde_json`。
  - 设计文档第 6.6、9、11 节和计划里 `release.rs`、`client.rs`、`lib.rs` 的代码已经跟着改了。
- **任务 12 的独立审查**：没有高危问题；1 个中等、10 个低优先级。改动如下：
  - **中等（已修）：两个窗口同时更新，可能把坏的 exe 换上去。**
    - 原因：两个窗口都下载到同一个 `.new`，后开始的会截断前一个正在写的文件；而哈希是边下载边算的，算的是数据流，不是磁盘上的文件。
    - 修法：
      - 下载前先删掉旧的 `.new`，再用 `create_new` 新建；Windows 上共享模式只给读。另一个窗口正在写时删不掉、打不开，提示“另一个 MhyCarry 窗口正在更新”（新错误 `Busy`）。
      - 替换前再读一遍 `.new` 核对哈希（`Staged::install`）。
      - 这样，打开程序时的清理也删不掉别的窗口正在下载的文件。
  - **低（已修）：**
    - 签名说明：程序核对 trusted comment 等于 `MhyCarry v<版本>`，手动运行发布流程（说明是分支名）签的文件冒充不了正式发布。`sign` job 也检查说明和签名格式（预先哈希，`ED`）。
    - 等着重启（`NeedsRestart`）时不能再检查更新（`UpdateView::can_check`），免得重装一遍时删 `.old` 失败。
    - `.old` 改到界面第一次取状态快照之后再删（`updater::clean_up_once`）。README 的回退说明补上“先删掉新的 exe”，并说明免安装版的文件名不会跟着变。
    - 下载中可以取消（新命令 `cancel_update`）。下载任务的进度和结果都先确认状态还是“下载中”，取消后不会再写回去；开始替换以后不能取消。
    - 更新中，“用米游社扫码绑定”和绑定结束后的“刷新”“重新绑定”也变灰。
    - 测试：`Model::begin_update`、`ensure_can_start_scan`、`cancel_download` 从命令里抽出来测；`UpdateView` 序列化后的结构；`.old` 删不掉时 `swap` 什么都不改。
    - 两处过时的注释（`release.yml` 的 job 数，`ci.yml` 不依赖平台的包）。
  - **没改：**
    - 签名私钥放进只允许 `v*` tag 的 Environment：要改仓库设置，先问用户。签名说明的核对已经挡住了手动运行签出的文件。
    - 刚更新完、`.old` 还没删就卸载时，安装文件夹会留下 `.old`：条件很窄，记一笔。
- **只能开一个**（2026-11-04 和用户确认）：审查的中等问题修完后，用户问多开有什么意义。
  - 结论：没有实际用处。多开时设置会互相覆盖（每个窗口整份写回自己内存里的那份），账号列表不同步，同一个码会被登录两次。
  - 改成单例，用官方插件 `tauri-plugin-single-instance` 2.5：再打开一次时把已经开着的窗口切到前面，新开的退出。它用到的 `tracing`、`windows-sys` 本来就在依赖里，`Cargo.lock` 只多了插件本身。
  - 自动更新重启前先调插件的 `destroy` 放开互斥量，否则新版本会以为已经有一个在运行而退出。这一步要等任务 13 的实测才能验证。
  - “另一个窗口正在更新”的保护留着：改成单例后基本碰不到，但代码很少，也有测试。
  - 实测：第二个约 0.2 秒退出，第一个从最小化恢复并切到前台，只剩一个进程。
