//! 直播客户端（设计文档第 9 节）：不带任何凭证和 Cookie。
//! 调平台接口时只能访问该平台的接口域名，不跟随跳转；拉流时可以访问接口返回的任意 HTTPS 地址，
//! 允许跳转（CDN 会重定向）。

use std::time::Duration;

use reqwest::header::{HeaderValue, REFERER};
use url::Url;

/// 浏览器的 User-Agent。平台接口可能拒绝没有 User-Agent 的请求。
const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0.0.0 Safari/537.36";
const API_TIMEOUT: Duration = Duration::from_secs(10);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, thiserror::Error)]
pub enum LiveError {
    #[error("网络请求失败：{0}")]
    Network(reqwest::Error),
    #[error("直播平台返回 HTTP {0}")]
    Status(u16),
    #[error("直播平台返回的内容格式不对，接口可能已变更：{0}")]
    Format(String),
    #[error("请求被拦截：{0}")]
    Blocked(String),
    #[error("找不到这个直播间")]
    RoomNotFound,
    #[error("这个直播间没有 H.264 的 FLV 直播流")]
    NoStream,
    #[error("{0}")]
    Other(String),
}

impl LiveError {
    /// reqwest 的错误信息带完整网址，流地址后面有一长串签名参数，先去掉。
    pub(crate) fn network(error: reqwest::Error) -> Self {
        Self::Network(error.without_url())
    }
}

pub struct LiveClient {
    api: reqwest::Client,
    stream: reqwest::Client,
    /// 测试时允许访问本地假服务器（http://127.0.0.1）。
    allow_local: bool,
}

impl LiveClient {
    pub fn new() -> Result<Self, LiveError> {
        Self::build(false)
    }

    #[cfg(test)]
    pub(crate) fn for_tests() -> Self {
        Self::build(true).expect("构建测试客户端")
    }

    fn build(allow_local: bool) -> Result<Self, LiveError> {
        let api = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .redirect(reqwest::redirect::Policy::none())
            .timeout(API_TIMEOUT)
            .https_only(!allow_local)
            .build()
            .map_err(LiveError::network)?;
        let stream = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .redirect(reqwest::redirect::Policy::limited(5))
            .connect_timeout(CONNECT_TIMEOUT)
            .https_only(!allow_local)
            .build()
            .map_err(LiveError::network)?;
        Ok(Self {
            api,
            stream,
            allow_local,
        })
    }

    /// 调平台接口，返回 JSON。`hosts` 是这个平台允许访问的接口域名。
    pub(crate) async fn get_json(
        &self,
        url: Url,
        hosts: &[&str],
        referer: &'static str,
    ) -> Result<serde_json::Value, LiveError> {
        let host = url.host_str().unwrap_or_default();
        let local = self.allow_local && matches!(host, "127.0.0.1" | "localhost");
        if !hosts.contains(&host) && !local {
            return Err(LiveError::Blocked(format!("不允许访问 {host}")));
        }
        let response = self
            .api
            .get(url)
            .header(REFERER, HeaderValue::from_static(referer))
            .send()
            .await
            .map_err(LiveError::network)?;
        if !response.status().is_success() {
            return Err(LiveError::Status(response.status().as_u16()));
        }
        let bytes = response.bytes().await.map_err(LiveError::network)?;
        serde_json::from_slice(&bytes).map_err(|error| LiveError::Format(error.to_string()))
    }

    /// 打开一个直播流地址。只允许 HTTPS。
    pub(crate) async fn open_stream(
        &self,
        url: &str,
        referer: &'static str,
    ) -> Result<reqwest::Response, LiveError> {
        let response = self
            .stream
            .get(url)
            .header(REFERER, HeaderValue::from_static(referer))
            .send()
            .await
            .map_err(LiveError::network)?;
        if !response.status().is_success() {
            return Err(LiveError::Status(response.status().as_u16()));
        }
        Ok(response)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn api_requests_only_go_to_the_platform_hosts() {
        let client = LiveClient::new().unwrap();
        let result = client
            .get_json(
                Url::parse("https://evil.cn/room").unwrap(),
                &["api.live.bilibili.com"],
                "https://live.bilibili.com/",
            )
            .await;
        assert!(matches!(result, Err(LiveError::Blocked(_))), "{result:?}");
    }

    #[tokio::test]
    async fn streams_must_be_https() {
        let client = LiveClient::new().unwrap();
        let result = client
            .open_stream("http://127.0.0.1:9/live.flv", "https://live.bilibili.com/")
            .await;
        assert!(matches!(result, Err(LiveError::Network(_))), "{result:?}");
    }
}
