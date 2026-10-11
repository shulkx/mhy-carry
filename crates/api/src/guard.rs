//! 域名白名单。每个请求发出前都要过这一关。

use url::Url;

use crate::endpoints::{PANDA_HOST, PASSPORT_HOST};
use crate::error::ApiError;

#[derive(Debug, Clone)]
pub struct HostPolicy {
    hosts: Vec<String>,
    require_https: bool,
}

impl HostPolicy {
    pub fn official() -> Self {
        Self {
            hosts: vec![PANDA_HOST.to_owned(), PASSPORT_HOST.to_owned()],
            require_https: true,
        }
    }

    /// 只允许本地假服务器，用于测试。
    #[cfg(test)]
    pub fn local_for_tests() -> Self {
        Self {
            hosts: vec!["127.0.0.1".to_owned()],
            require_https: false,
        }
    }

    pub fn require_https(&self) -> bool {
        self.require_https
    }

    pub fn check(&self, url: &Url) -> Result<(), ApiError> {
        if self.require_https && url.scheme() != "https" {
            return Err(ApiError::Blocked(format!(
                "只允许 https，实际是 {}",
                url.scheme()
            )));
        }
        if !url.username().is_empty() || url.password().is_some() {
            return Err(ApiError::Blocked("地址里不能带用户名或密码".to_owned()));
        }
        let host = url.host_str().unwrap_or_default();
        if !self.hosts.iter().any(|allowed| allowed == host) {
            return Err(ApiError::Blocked(format!("域名不在白名单里：{host}")));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(url: &str) -> Result<(), ApiError> {
        HostPolicy::official().check(&Url::parse(url).unwrap())
    }

    #[test]
    fn allows_official_hosts_over_https() {
        assert!(check("https://api-sdk.mihoyo.com/hk4e_cn/combo/panda/qrcode/scan").is_ok());
        assert!(check("https://passport-api.mihoyo.com/account/x").is_ok());
    }

    #[test]
    fn blocks_plain_http() {
        assert!(matches!(
            check("http://passport-api.mihoyo.com/account/x"),
            Err(ApiError::Blocked(_))
        ));
    }

    #[test]
    fn blocks_other_hosts() {
        for url in [
            "https://evil.cn/x",
            "https://passport-api.mihoyo.com.evil.cn/x",
            "https://api-takumi.mihoyo.com/x",
            "https://127.0.0.1/x",
        ] {
            assert!(matches!(check(url), Err(ApiError::Blocked(_))), "{url}");
        }
    }

    #[test]
    fn blocks_userinfo() {
        assert!(matches!(
            check("https://a@passport-api.mihoyo.com/x"),
            Err(ApiError::Blocked(_))
        ));
    }
}
