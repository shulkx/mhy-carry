//! ApiClient：所有请求都从这里发出。

use std::time::Duration;

use reqwest::header::{CONTENT_TYPE, HeaderMap, HeaderValue};
use serde::Serialize;
use serde::de::{DeserializeOwned, IgnoredAny};
use url::Url;

use crate::endpoints::Endpoints;
use crate::envelope::Envelope;
use crate::error::ApiError;
use crate::guard::HostPolicy;

const TIMEOUT: Duration = Duration::from_secs(10);

pub struct ApiClient {
    pub(crate) http: reqwest::Client,
    pub(crate) policy: HostPolicy,
    pub(crate) endpoints: Endpoints,
    pub(crate) device_id: String,
}

impl ApiClient {
    /// 正式客户端：只能访问官方域名，只用 https，不跟随重定向。
    pub fn new(device_id: impl Into<String>) -> Result<Self, ApiError> {
        Self::build(
            device_id.into(),
            Endpoints::official(),
            HostPolicy::official(),
        )
    }

    /// 测试客户端：所有接口指向本地假服务器。
    #[cfg(test)]
    pub(crate) fn for_tests(base: &str) -> Self {
        Self::build(
            "test-device".to_owned(),
            Endpoints::all_at(base),
            HostPolicy::local_for_tests(),
        )
        .expect("构建测试客户端")
    }

    fn build(
        device_id: String,
        endpoints: Endpoints,
        policy: HostPolicy,
    ) -> Result<Self, ApiError> {
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(TIMEOUT)
            .https_only(policy.require_https())
            .build()
            .map_err(ApiError::network)?;
        Ok(Self {
            http,
            policy,
            endpoints,
            device_id,
        })
    }

    pub(crate) fn device_header(&self) -> Result<HeaderValue, ApiError> {
        HeaderValue::from_str(&self.device_id)
            .map_err(|_| ApiError::Format("设备 ID 里有非法字符".to_owned()))
    }

    pub(crate) async fn post_json<B: Serialize, T: DeserializeOwned>(
        &self,
        url: Url,
        headers: HeaderMap,
        body: &B,
    ) -> Result<Envelope<T>, ApiError> {
        let body = serde_json::to_string(body).map_err(|e| ApiError::Format(e.to_string()))?;
        self.post_text(url, headers, body).await
    }

    /// 发送已经序列化好的 JSON 请求体。需要对请求体签名时（任务 19）用得上。
    pub(crate) async fn post_text<T: DeserializeOwned>(
        &self,
        url: Url,
        headers: HeaderMap,
        body: String,
    ) -> Result<Envelope<T>, ApiError> {
        self.policy.check(&url)?;
        let response = self
            .http
            .post(url)
            .headers(headers)
            .header(CONTENT_TYPE, "application/json")
            .body(body)
            .send()
            .await
            .map_err(ApiError::network)?;
        read_envelope(response).await
    }

    pub(crate) async fn get_json<T: DeserializeOwned>(
        &self,
        url: Url,
        headers: HeaderMap,
    ) -> Result<Envelope<T>, ApiError> {
        self.policy.check(&url)?;
        let response = self
            .http
            .get(url)
            .headers(headers)
            .send()
            .await
            .map_err(ApiError::network)?;
        read_envelope(response).await
    }
}

async fn read_envelope<T: DeserializeOwned>(
    response: reqwest::Response,
) -> Result<Envelope<T>, ApiError> {
    let status = response.status();
    if !status.is_success() {
        return Err(ApiError::Status(status.as_u16()));
    }
    let bytes = response.bytes().await.map_err(ApiError::network)?;
    // 先只看 retcode：出错时 data 的结构可能和成功时不同，不能让它掩盖错误码。
    serde_json::from_slice::<Envelope<IgnoredAny>>(&bytes)
        .map_err(format_error)?
        .ensure_ok()?;
    serde_json::from_slice(&bytes).map_err(format_error)
}

/// 返回内容解析失败。serde 的类型、取值错误会带上字段原值（可能就是 stoken），
/// 这类错误只报位置；缺字段的错误只含字段名，保留下来方便排查接口变化。
fn format_error(error: serde_json::Error) -> ApiError {
    let text = error.to_string();
    if error.is_data() && !text.starts_with("missing field") {
        return ApiError::Format(format!(
            "字段类型或取值不对（第 {} 行第 {} 列）",
            error.line(),
            error.column()
        ));
    }
    ApiError::Format(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::de::IgnoredAny;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    async fn post_to(client: &ApiClient, url: Url) -> Result<Envelope<IgnoredAny>, ApiError> {
        client
            .post_json(url, HeaderMap::new(), &serde_json::json!({}))
            .await
    }

    #[tokio::test]
    async fn does_not_follow_redirects() {
        let evil = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200))
            .expect(0)
            .mount(&evil)
            .await;
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/x"))
            .respond_with(
                ResponseTemplate::new(302)
                    .insert_header("location", format!("{}/steal", evil.uri())),
            )
            .mount(&server)
            .await;

        let client = ApiClient::for_tests(&server.uri());
        let result = post_to(&client, client.endpoints.passport("/x")).await;
        assert!(matches!(result, Err(ApiError::Status(302))), "{result:?}");
    }

    #[tokio::test]
    async fn http_errors_are_reported() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&server)
            .await;
        let client = ApiClient::for_tests(&server.uri());
        let result = post_to(&client, client.endpoints.passport("/x")).await;
        assert!(matches!(result, Err(ApiError::Status(500))), "{result:?}");
    }

    #[tokio::test]
    async fn non_json_body_is_a_format_error() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_string("<html>维护中</html>"))
            .mount(&server)
            .await;
        let client = ApiClient::for_tests(&server.uri());
        let result = post_to(&client, client.endpoints.passport("/x")).await;
        assert!(matches!(result, Err(ApiError::Format(_))), "{result:?}");
    }

    #[tokio::test]
    async fn official_client_blocks_other_hosts_before_sending() {
        let client = ApiClient::new("device").unwrap();
        let result = post_to(&client, Url::parse("https://evil.cn/x").unwrap()).await;
        assert!(matches!(result, Err(ApiError::Blocked(_))), "{result:?}");
    }

    /// 用一个要求 data 是数组的类型去解析，制造 data 格式不对的情况。
    async fn post_expecting_array(server: &MockServer, body: serde_json::Value) -> ApiError {
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(body))
            .mount(server)
            .await;
        let client = ApiClient::for_tests(&server.uri());
        let result: Result<Envelope<Vec<u8>>, ApiError> = client
            .post_json(
                client.endpoints.passport("/x"),
                HeaderMap::new(),
                &serde_json::json!({}),
            )
            .await;
        result.unwrap_err()
    }

    #[tokio::test]
    async fn retcode_is_reported_even_if_data_is_malformed() {
        let server = MockServer::start().await;
        let body = serde_json::json!({"retcode": -3501, "message": "二维码已过期", "data": {}});
        let error = post_expecting_array(&server, body).await;
        assert_eq!(error.retcode(), Some(-3501), "{error:?}");
    }

    #[tokio::test]
    async fn format_errors_do_not_echo_values_from_the_response() {
        let server = MockServer::start().await;
        let body = serde_json::json!({"retcode": 0, "message": "OK", "data": "v2_secret_stoken"});
        let error = post_expecting_array(&server, body).await;
        let text = format!("{error} {error:?}");
        assert!(matches!(error, ApiError::Format(_)), "{text}");
        assert!(!text.contains("v2_secret_stoken"), "{text}");
    }
}
