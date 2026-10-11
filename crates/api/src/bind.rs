//! 绑定账号：米哈游启动器（HoYoPlay）的 passport 扫码登录（设计文档第 6.1 节）。

use mhy_carry_core::account::SecretString;
use reqwest::header::{HeaderMap, HeaderValue, USER_AGENT};
use serde::de::Error as _;
use serde::{Deserialize, Deserializer};
use serde_json::{Value, json};

use crate::client::ApiClient;
use crate::error::ApiError;

const HYP_APP_ID: &str = "ddxf5dufpuyo";
const HYP_CLIENT_TYPE: &str = "3";
const HYP_USER_AGENT: &str = "HYPContainer/1.1.4.133";
const CREATE_PATH: &str = "/account/ma-cn-passport/app/createQRLogin";
const QUERY_PATH: &str = "/account/ma-cn-passport/app/queryQRLoginStatus";
/// `tokens` 里 `token_type` 为 1 的是 stoken。
const STOKEN_TYPE: i64 = 1;

/// 给米游社 App 扫的绑定二维码。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BindQr {
    /// 二维码内容。
    pub url: String,
    /// 查询状态用的票据。
    pub ticket: String,
}

#[derive(Debug)]
pub enum BindStatus {
    /// 还没扫。
    Created,
    /// 已扫码，等手机上确认。
    Scanned,
    Confirmed(BoundAccount),
}

#[derive(Debug)]
pub struct BoundAccount {
    pub uid: String,
    pub mid: String,
    pub stoken: SecretString,
}

#[derive(Deserialize)]
struct CreateData {
    url: String,
    ticket: String,
}

#[derive(Deserialize)]
struct QueryData {
    status: String,
    #[serde(default)]
    tokens: Vec<TokenItem>,
    #[serde(default)]
    user_info: Option<UserInfo>,
}

#[derive(Deserialize)]
struct TokenItem {
    #[serde(deserialize_with = "int_or_string")]
    token_type: i64,
    token: String,
}

#[derive(Deserialize)]
struct UserInfo {
    #[serde(deserialize_with = "string_or_int")]
    aid: String,
    mid: String,
}

impl ApiClient {
    fn hyp_headers(&self) -> Result<HeaderMap, ApiError> {
        let mut headers = HeaderMap::new();
        headers.insert("x-rpc-app_id", HeaderValue::from_static(HYP_APP_ID));
        headers.insert(
            "x-rpc-client_type",
            HeaderValue::from_static(HYP_CLIENT_TYPE),
        );
        headers.insert("x-rpc-device_id", self.device_header()?);
        headers.insert(USER_AGENT, HeaderValue::from_static(HYP_USER_AGENT));
        Ok(headers)
    }

    pub async fn create_bind_qr(&self) -> Result<BindQr, ApiError> {
        let url = self.endpoints.passport(CREATE_PATH);
        let data: CreateData = self
            .post_json(url, self.hyp_headers()?, &json!({}))
            .await?
            .into_data()?;
        Ok(BindQr {
            url: data.url,
            ticket: data.ticket,
        })
    }

    pub async fn query_bind_qr(&self, ticket: &str) -> Result<BindStatus, ApiError> {
        let url = self.endpoints.passport(QUERY_PATH);
        let data: QueryData = self
            .post_json(url, self.hyp_headers()?, &json!({ "ticket": ticket }))
            .await?
            .into_data()?;
        match data.status.as_str() {
            "Created" | "Init" => Ok(BindStatus::Created),
            "Scanned" => Ok(BindStatus::Scanned),
            "Confirmed" => {
                let stoken = data
                    .tokens
                    .into_iter()
                    .find(|t| t.token_type == STOKEN_TYPE)
                    .ok_or_else(|| ApiError::Format("确认后的返回里没有 stoken".to_owned()))?;
                let user = data
                    .user_info
                    .ok_or_else(|| ApiError::Format("确认后的返回里没有 user_info".to_owned()))?;
                Ok(BindStatus::Confirmed(BoundAccount {
                    uid: user.aid,
                    mid: user.mid,
                    stoken: SecretString::from(stoken.token),
                }))
            }
            other => Err(ApiError::Format(format!("未知的二维码状态：{other}"))),
        }
    }
}

fn string_or_int<'de, D: Deserializer<'de>>(deserializer: D) -> Result<String, D::Error> {
    match Value::deserialize(deserializer)? {
        Value::String(s) => Ok(s),
        Value::Number(n) => Ok(n.to_string()),
        _ => Err(D::Error::custom("应为字符串或数字")),
    }
}

fn int_or_string<'de, D: Deserializer<'de>>(deserializer: D) -> Result<i64, D::Error> {
    match Value::deserialize(deserializer)? {
        Value::Number(n) => n.as_i64().ok_or_else(|| D::Error::custom("数字超出范围")),
        Value::String(s) => s.parse().map_err(|_| D::Error::custom("不是整数")),
        _ => Err(D::Error::custom("应为整数或字符串")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mhy_carry_core::account::ExposeSecret;
    use wiremock::matchers::{body_json, header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn ok(data: serde_json::Value) -> ResponseTemplate {
        ResponseTemplate::new(200)
            .set_body_json(json!({"retcode": 0, "message": "OK", "data": data}))
    }

    async fn server_with_query_responses(responses: Vec<ResponseTemplate>) -> MockServer {
        let server = MockServer::start().await;
        // 先挂上的先匹配；每个只用一次，于是按顺序依次返回。
        for response in responses {
            Mock::given(method("POST"))
                .and(path(QUERY_PATH))
                .and(body_json(json!({"ticket": "t1"})))
                .respond_with(response)
                .up_to_n_times(1)
                .mount(&server)
                .await;
        }
        server
    }

    #[tokio::test]
    async fn create_sends_hoyoplay_headers() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path(CREATE_PATH))
            .and(header("x-rpc-app_id", "ddxf5dufpuyo"))
            .and(header("x-rpc-client_type", "3"))
            .and(header("x-rpc-device_id", "test-device"))
            .and(header("user-agent", "HYPContainer/1.1.4.133"))
            .and(body_json(json!({})))
            .respond_with(ok(
                json!({"url": "https://user.mihoyo.com/login-platform/x", "ticket": "t1"}),
            ))
            .expect(1)
            .mount(&server)
            .await;
        let client = ApiClient::for_tests(&server.uri());
        let qr = client.create_bind_qr().await.unwrap();
        assert_eq!(qr.ticket, "t1");
        assert_eq!(qr.url, "https://user.mihoyo.com/login-platform/x");
    }

    #[tokio::test]
    async fn query_follows_status_until_confirmed() {
        let server = server_with_query_responses(vec![
            ok(json!({"status": "Created"})),
            ok(json!({"status": "Scanned"})),
            ok(json!({
                "status": "Confirmed",
                "tokens": [{"token_type": 1, "token": "v2_new_stoken"}],
                "user_info": {"aid": "123456789", "mid": "mid_abc"}
            })),
        ])
        .await;
        let client = ApiClient::for_tests(&server.uri());

        assert!(matches!(
            client.query_bind_qr("t1").await.unwrap(),
            BindStatus::Created
        ));
        assert!(matches!(
            client.query_bind_qr("t1").await.unwrap(),
            BindStatus::Scanned
        ));
        let BindStatus::Confirmed(account) = client.query_bind_qr("t1").await.unwrap() else {
            panic!("应该是 Confirmed");
        };
        assert_eq!(account.uid, "123456789");
        assert_eq!(account.mid, "mid_abc");
        assert_eq!(account.stoken.expose_secret(), "v2_new_stoken");
    }

    #[tokio::test]
    async fn confirmed_accepts_numeric_aid_and_string_token_type() {
        let server = server_with_query_responses(vec![ok(json!({
            "status": "Confirmed",
            "tokens": [{"token_type": "2", "token": "other"}, {"token_type": "1", "token": "v2_s"}],
            "user_info": {"aid": 123456789, "mid": "m"}
        }))])
        .await;
        let client = ApiClient::for_tests(&server.uri());
        let BindStatus::Confirmed(account) = client.query_bind_qr("t1").await.unwrap() else {
            panic!("应该是 Confirmed");
        };
        assert_eq!(account.uid, "123456789");
        assert_eq!(account.stoken.expose_secret(), "v2_s");
    }

    #[tokio::test]
    async fn confirmed_without_stoken_is_a_format_error() {
        let server = server_with_query_responses(vec![ok(json!({
            "status": "Confirmed",
            "tokens": [],
            "user_info": {"aid": "1", "mid": "m"}
        }))])
        .await;
        let client = ApiClient::for_tests(&server.uri());
        assert!(matches!(
            client.query_bind_qr("t1").await,
            Err(ApiError::Format(_))
        ));
    }

    #[tokio::test]
    async fn missing_field_error_names_the_field() {
        let server = server_with_query_responses(vec![ok(json!({}))]).await;
        let client = ApiClient::for_tests(&server.uri());
        let error = client.query_bind_qr("t1").await.unwrap_err();
        assert!(
            matches!(&error, ApiError::Format(m) if m.contains("status")),
            "{error}"
        );
    }

    #[tokio::test]
    async fn unknown_status_is_a_format_error() {
        let server = server_with_query_responses(vec![ok(json!({"status": "Whatever"}))]).await;
        let client = ApiClient::for_tests(&server.uri());
        assert!(matches!(
            client.query_bind_qr("t1").await,
            Err(ApiError::Format(_))
        ));
    }

    #[tokio::test]
    async fn expired_bind_qr_reports_its_retcode() {
        let server =
            server_with_query_responses(vec![ResponseTemplate::new(200).set_body_json(
                json!({"retcode": -3501, "message": "二维码已过期", "data": null}),
            )])
            .await;
        let client = ApiClient::for_tests(&server.uri());
        let error = client.query_bind_qr("t1").await.unwrap_err();
        assert_eq!(error.retcode(), Some(crate::retcode::QR_EXPIRED));
    }
}
