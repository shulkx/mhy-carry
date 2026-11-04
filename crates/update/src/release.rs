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
    let tag = url.path().strip_prefix(&format!("/{REPO}/releases/tag/"))?;
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
            assert_eq!(
                version_from_location(&site(), location),
                Some(V041),
                "{location}"
            );
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
