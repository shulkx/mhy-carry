//! 扫码登录：检查凭证，以及 panda 扫码 → passport 扫码 → 确认 三步（设计文档第 6.2 节）。

use mhy_carry_core::account::{ExposeSecret, SecretString};
use mhy_carry_core::qr::{LoginQr, PassportTicket, parse_passport_url};
use mhy_carry_core::scan::Failure;
use mhy_carry_core::text::printable;
use reqwest::header::{COOKIE, HeaderMap, HeaderValue};
use serde::Deserialize;
use serde::de::IgnoredAny;
use serde_json::json;

use crate::client::ApiClient;
use crate::error::{ApiError, retcode};

/// 米游社 App 的应用 ID。
const BBS_APP_ID: &str = "bll8iq97cem8";
const CHECK_STOKEN_PATH: &str = "/account/auth/api/getCookieAccountInfoBySToken";
const SCAN_PATH: &str = "/account/ma-cn-passport/app/scanQRLogin";
const CONFIRM_PATH: &str = "/account/ma-cn-passport/app/confirmQRLogin";

/// 账号凭证：v2 stoken 和 mid。
pub struct Credentials<'a> {
    pub stoken: &'a SecretString,
    pub mid: &'a str,
}

/// passport 扫码后，服务器认定的登录目标应用。
#[derive(Debug, Clone, Deserialize)]
pub struct ScannedApp {
    #[serde(default)]
    pub app_id: String,
    #[serde(default)]
    pub app_name: String,
}

#[derive(Deserialize)]
struct PandaScanData {
    passport_qr_url: String,
}

#[derive(Deserialize)]
struct CookieTokenData {
    #[serde(default)]
    cookie_token: Option<SecretString>,
}

/// 扫码登录失败。
#[derive(Debug, thiserror::Error)]
pub enum LoginError {
    #[error(transparent)]
    Api(#[from] ApiError),
    /// 服务器认定的登录目标不是二维码对应的游戏。已经取消，没有调用确认。
    #[error("服务器说这个二维码是“{app_name}”（{app_id}）的登录码，不是{expected}，已取消登录")]
    AppMismatch {
        app_id: String,
        app_name: String,
        expected: &'static str,
    },
}

/// 把接口错误分成扫码会话需要的几类（设计文档第 10 节）。
impl From<ApiError> for Failure {
    fn from(error: ApiError) -> Self {
        match error.retcode() {
            Some(retcode::TOKEN_INVALID) => Failure::CredentialInvalid,
            Some(retcode::QR_EXPIRED) => Failure::QrExpired,
            _ => Failure::Other(error.to_string()),
        }
    }
}

impl From<LoginError> for Failure {
    fn from(error: LoginError) -> Self {
        match error {
            LoginError::Api(error) => error.into(),
            LoginError::AppMismatch { app_name, .. } => Failure::AppMismatch { app_name },
        }
    }
}

impl ApiClient {
    fn bbs_headers(&self, credentials: Option<&Credentials<'_>>) -> Result<HeaderMap, ApiError> {
        let mut headers = HeaderMap::new();
        headers.insert("x-rpc-app_id", HeaderValue::from_static(BBS_APP_ID));
        headers.insert("x-rpc-device_id", self.device_header()?);
        if let Some(credentials) = credentials {
            let cookie = format!(
                "stoken={};mid={}",
                credentials.stoken.expose_secret(),
                credentials.mid
            );
            let mut cookie = HeaderValue::from_str(&cookie)
                .map_err(|_| ApiError::Format("账号凭证里有非法字符".to_owned()))?;
            cookie.set_sensitive(true);
            headers.insert(COOKIE, cookie);
        }
        Ok(headers)
    }

    /// 检查 stoken 是否仍然有效（设计文档第 6.2 节），顺便取回 cookie_token：查原神角色要用它
    /// （第 6.4 节）。cookie_token 也是凭证，只在内存里用。返回内容里没有它时返回 None，
    /// 只当作检查通过。
    pub async fn check_stoken(
        &self,
        credentials: &Credentials<'_>,
    ) -> Result<Option<SecretString>, ApiError> {
        let url = self.endpoints.passport(CHECK_STOKEN_PATH);
        let data: Option<CookieTokenData> = self
            .get_json(url, self.bbs_headers(Some(credentials))?)
            .await?
            .into_optional_data()?;
        Ok(data.and_then(|data| data.cookie_token))
    }

    /// 第 1 步：用游戏二维码的 ticket 调 panda 扫码，换来 passport 二维码的票据。
    pub async fn panda_scan(&self, qr: &LoginQr) -> Result<PassportTicket, ApiError> {
        let url = self.endpoints.panda_scan(qr.game.biz_key);
        let body = json!({
            "passport_app_id": BBS_APP_ID,
            "ticket": qr.ticket,
            "app_id": qr.game.app_id,
            "device": self.device_id,
            "ts": jiff::Timestamp::now().as_second(),
        });
        let data: PandaScanData = self
            .post_json(url, self.bbs_headers(None)?, &body)
            .await?
            .into_data()?;
        parse_passport_url(&data.passport_qr_url)
            .map_err(|e| ApiError::Format(format!("passport_qr_url 不合法：{e}")))
    }

    /// 第 2 步：passport 扫码。返回服务器认定的登录目标应用，确认前要核对。
    pub async fn passport_scan(
        &self,
        ticket: &PassportTicket,
        credentials: &Credentials<'_>,
    ) -> Result<ScannedApp, ApiError> {
        let url = self.endpoints.passport(SCAN_PATH);
        self.post_json(
            url,
            self.bbs_headers(Some(credentials))?,
            &passport_body(ticket),
        )
        .await?
        .into_data()
    }

    /// 第 3 步：确认登录。
    pub async fn passport_confirm(
        &self,
        ticket: &PassportTicket,
        credentials: &Credentials<'_>,
    ) -> Result<(), ApiError> {
        let url = self.endpoints.passport(CONFIRM_PATH);
        self.post_json::<_, IgnoredAny>(
            url,
            self.bbs_headers(Some(credentials))?,
            &passport_body(ticket),
        )
        .await?
        .ensure_ok()
    }

    /// 扫码登录：panda 扫码 → passport 扫码 → 核对应用 → 确认（设计文档第 6.2 节第 6 步）。
    /// 服务器返回的应用 ID 和二维码对应的游戏对不上时，不调用确认。
    pub async fn login_with_qr(
        &self,
        qr: &LoginQr,
        credentials: &Credentials<'_>,
    ) -> Result<(), LoginError> {
        let ticket = self.panda_scan(qr).await?;
        let app = self.passport_scan(&ticket, credentials).await?;
        if app.app_id != qr.game.passport_app_id {
            return Err(LoginError::AppMismatch {
                app_id: printable(&app.app_id),
                app_name: printable(&app.app_name),
                expected: qr.game.display_name,
            });
        }
        self.passport_confirm(&ticket, credentials).await?;
        Ok(())
    }
}

fn passport_body(ticket: &PassportTicket) -> serde_json::Value {
    json!({ "ticket": ticket.tk, "token_types": ticket.token_types })
}

#[cfg(test)]
mod tests {
    use super::*;
    use mhy_carry_core::qr::parse_login_qr;
    use wiremock::matchers::{body_json, body_partial_json, header, method, path};
    use wiremock::{Mock, MockServer, Request, ResponseTemplate};

    const TK: &str = "4f1c2a9e-7b3d-4c55-9a1e-0d2b6f8c9e10";

    fn login_qr() -> LoginQr {
        parse_login_qr(
            "https://user.mihoyo.com/qr_code_in_game.html?app_id=4&biz_key=hk4e_cn&expire=4102444800&ticket=66cf3afb2a1c7a308832bd56",
            1_760_000_000,
        )
        .unwrap()
    }

    fn passport_ticket() -> PassportTicket {
        PassportTicket {
            tk: TK.to_owned(),
            token_types: vec!["1".to_owned()],
        }
    }

    fn ok(data: serde_json::Value) -> ResponseTemplate {
        ResponseTemplate::new(200)
            .set_body_json(json!({"retcode": 0, "message": "OK", "data": data}))
    }

    fn fail(code: i64) -> ResponseTemplate {
        ResponseTemplate::new(200)
            .set_body_json(json!({"retcode": code, "message": "失败", "data": null}))
    }

    #[tokio::test]
    async fn panda_scan_sends_expected_request_and_parses_passport_url() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/hk4e_cn/combo/panda/qrcode/scan"))
            .and(header("x-rpc-app_id", "bll8iq97cem8"))
            .and(header("x-rpc-device_id", "test-device"))
            .and(body_partial_json(json!({
                "passport_app_id": "bll8iq97cem8",
                "ticket": "66cf3afb2a1c7a308832bd56",
                "app_id": 4,
                "device": "test-device",
            })))
            .and(|req: &Request| {
                let body: serde_json::Value = serde_json::from_slice(&req.body).unwrap();
                body["ts"].is_i64() && body["app_id"].is_u64()
            })
            .respond_with(ok(json!({
                "passport_qr_url": format!("https://user.mihoyo.com/login-platform/mobile.html?expire=1760000300&tk={TK}&token_types=1#/login/qr")
            })))
            .expect(1)
            .mount(&server)
            .await;

        let client = ApiClient::for_tests(&server.uri());
        let got = client.panda_scan(&login_qr()).await.unwrap();
        assert_eq!(got, passport_ticket());
    }

    #[tokio::test]
    async fn panda_scan_rejects_untrusted_passport_url() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ok(json!({
                "passport_qr_url": format!("https://evil.cn/login-platform/mobile.html?tk={TK}&token_types=1")
            })))
            .mount(&server)
            .await;
        let client = ApiClient::for_tests(&server.uri());
        let result = client.panda_scan(&login_qr()).await;
        assert!(matches!(result, Err(ApiError::Format(_))), "{result:?}");
    }

    #[tokio::test]
    async fn passport_scan_and_confirm_use_cookie_auth() {
        let server = MockServer::start().await;
        let expected_body = json!({"ticket": TK, "token_types": ["1"]});
        Mock::given(method("POST"))
            .and(path(SCAN_PATH))
            .and(header("cookie", "stoken=v2_secret_stoken;mid=mid_abc"))
            .and(header("x-rpc-app_id", "bll8iq97cem8"))
            .and(body_json(&expected_body))
            .respond_with(ok(
                json!({"app_id": "c76ync6mutq8", "app_name": "原神", "client_type": 3}),
            ))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path(CONFIRM_PATH))
            .and(header("cookie", "stoken=v2_secret_stoken;mid=mid_abc"))
            .and(body_json(&expected_body))
            .respond_with(ok(json!({})))
            .expect(1)
            .mount(&server)
            .await;

        let client = ApiClient::for_tests(&server.uri());
        let stoken = SecretString::from("v2_secret_stoken");
        let credentials = Credentials {
            stoken: &stoken,
            mid: "mid_abc",
        };
        let app = client
            .passport_scan(&passport_ticket(), &credentials)
            .await
            .unwrap();
        assert_eq!(app.app_id, "c76ync6mutq8");
        assert_eq!(app.app_name, "原神");
        client
            .passport_confirm(&passport_ticket(), &credentials)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn expired_qr_reports_its_retcode() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path(SCAN_PATH))
            .respond_with(fail(crate::retcode::QR_EXPIRED))
            .mount(&server)
            .await;
        let client = ApiClient::for_tests(&server.uri());
        let stoken = SecretString::from("v2_secret_stoken");
        let credentials = Credentials {
            stoken: &stoken,
            mid: "mid_abc",
        };
        let error = client
            .passport_scan(&passport_ticket(), &credentials)
            .await
            .unwrap_err();
        assert_eq!(error.retcode(), Some(crate::retcode::QR_EXPIRED));
    }

    #[tokio::test]
    async fn check_stoken_accepts_valid_and_reports_invalid() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(CHECK_STOKEN_PATH))
            .and(header("cookie", "stoken=good;mid=mid_abc"))
            .respond_with(ok(json!({"cookie_token": "x"})))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path(CHECK_STOKEN_PATH))
            .and(header("cookie", "stoken=bad;mid=mid_abc"))
            .respond_with(fail(crate::retcode::TOKEN_INVALID))
            .mount(&server)
            .await;
        let client = ApiClient::for_tests(&server.uri());

        let good = SecretString::from("good");
        let good = Credentials {
            stoken: &good,
            mid: "mid_abc",
        };
        let token = client.check_stoken(&good).await.unwrap();
        assert_eq!(token.unwrap().expose_secret(), "x");

        let bad = SecretString::from("bad");
        let bad = Credentials {
            stoken: &bad,
            mid: "mid_abc",
        };
        let error = client.check_stoken(&bad).await.unwrap_err();
        assert_eq!(error.retcode(), Some(crate::retcode::TOKEN_INVALID));
    }

    #[tokio::test]
    async fn check_stoken_without_cookie_token_still_passes() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(CHECK_STOKEN_PATH))
            .respond_with(ok(json!({"uid": "300012345"})))
            .mount(&server)
            .await;
        let client = ApiClient::for_tests(&server.uri());
        let stoken = SecretString::from("good");
        let credentials = Credentials {
            stoken: &stoken,
            mid: "mid_abc",
        };
        assert!(client.check_stoken(&credentials).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn errors_never_contain_the_stoken() {
        // 拿一个刚释放的端口，保证连接被拒绝。
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        drop(listener);

        let client = ApiClient::for_tests(&format!("http://{addr}"));
        let stoken = SecretString::from("v2_secret_stoken");
        let credentials = Credentials {
            stoken: &stoken,
            mid: "mid_abc",
        };
        let error = client
            .passport_confirm(&passport_ticket(), &credentials)
            .await
            .unwrap_err();
        let text = format!("{error} {error:?}");
        assert!(matches!(error, ApiError::Network(_)), "{text}");
        assert!(!text.contains("v2_secret_stoken"), "{text}");
    }

    #[tokio::test]
    async fn login_with_qr_scans_checks_the_app_and_confirms() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/hk4e_cn/combo/panda/qrcode/scan"))
            .respond_with(ok(json!({
                "passport_qr_url": format!("https://user.mihoyo.com/login-platform/mobile.html?tk={TK}&token_types=1#/login/qr")
            })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path(SCAN_PATH))
            .respond_with(ok(json!({"app_id": "c76ync6mutq8", "app_name": "原神"})))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path(CONFIRM_PATH))
            .respond_with(ok(json!({})))
            .expect(1)
            .mount(&server)
            .await;
        let client = ApiClient::for_tests(&server.uri());
        let stoken = SecretString::from("v2_secret_stoken");
        let credentials = Credentials {
            stoken: &stoken,
            mid: "mid_abc",
        };
        client
            .login_with_qr(&login_qr(), &credentials)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn login_with_qr_never_confirms_a_different_app() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/hk4e_cn/combo/panda/qrcode/scan"))
            .respond_with(ok(json!({
                "passport_qr_url": format!("https://user.mihoyo.com/login-platform/mobile.html?tk={TK}&token_types=1#/login/qr")
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path(SCAN_PATH))
            .respond_with(ok(
                json!({"app_id": "other_app", "app_name": "崩坏：星穹铁道"}),
            ))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path(CONFIRM_PATH))
            .respond_with(ok(json!({})))
            .expect(0)
            .mount(&server)
            .await;
        let client = ApiClient::for_tests(&server.uri());
        let stoken = SecretString::from("v2_secret_stoken");
        let credentials = Credentials {
            stoken: &stoken,
            mid: "mid_abc",
        };
        let error = client
            .login_with_qr(&login_qr(), &credentials)
            .await
            .unwrap_err();
        assert!(
            matches!(&error, LoginError::AppMismatch { app_id, .. } if app_id == "other_app"),
            "{error}"
        );
    }

    #[test]
    fn errors_are_classified_for_the_scan_session() {
        let retcode = |code| ApiError::Retcode {
            code,
            message: "x".to_owned(),
        };
        assert_eq!(Failure::from(retcode(-100)), Failure::CredentialInvalid);
        assert_eq!(Failure::from(retcode(-3501)), Failure::QrExpired);
        assert!(matches!(
            Failure::from(ApiError::Status(500)),
            Failure::Other(message) if message.contains("500")
        ));
        let mismatch = LoginError::AppMismatch {
            app_id: "other_app".to_owned(),
            app_name: "崩坏：星穹铁道".to_owned(),
            expected: "原神",
        };
        assert_eq!(
            Failure::from(mismatch),
            Failure::AppMismatch {
                app_name: "崩坏：星穹铁道".to_owned()
            }
        );
    }
}
