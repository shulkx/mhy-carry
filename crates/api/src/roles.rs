//! 查原神角色（设计文档第 6.4 节）：用检查凭证时拿到的 cookie_token，查这个通行证在原神官服的
//! 角色。接口在 passport-api 上，只带 Cookie，不需要 DS 签名（核对来源见第 16 节）。

use mhy_carry_core::account::{ExposeSecret, GameRole, SecretString};
use mhy_carry_core::text::printable;
use reqwest::header::{COOKIE, HeaderMap, HeaderValue, REFERER};
use serde::Deserialize;

use crate::client::ApiClient;
use crate::error::ApiError;

const ROLES_PATH: &str = "/binding/api/getUserGameRolesByCookieToken";
/// 原神国服。
const GENSHIN_BIZ: &str = "hk4e_cn";
/// 官服。B 服是 `cn_qd01`，v1 不支持。
const OFFICIAL_REGION: &str = "cn_gf01";

#[derive(Deserialize)]
struct RoleList {
    #[serde(default)]
    list: Vec<RoleItem>,
}

#[derive(Deserialize)]
struct RoleItem {
    #[serde(default)]
    region: String,
    #[serde(default)]
    game_uid: String,
    #[serde(default)]
    nickname: String,
    #[serde(default)]
    level: u32,
}

impl ApiClient {
    /// 查通行证 `account_id` 在原神官服的角色；有多个时取等级最高的，一个都没有时返回 None。
    /// `cookie_token` 来自 [`ApiClient::check_stoken`]。
    pub async fn genshin_role(
        &self,
        account_id: &str,
        cookie_token: &SecretString,
    ) -> Result<Option<GameRole>, ApiError> {
        let mut url = self.endpoints.passport(ROLES_PATH);
        url.query_pairs_mut().append_pair("game_biz", GENSHIN_BIZ);
        let cookie = format!(
            "account_id={account_id}; cookie_token={}",
            cookie_token.expose_secret()
        );
        let mut cookie = HeaderValue::from_str(&cookie)
            .map_err(|_| ApiError::Format("账号凭证里有非法字符".to_owned()))?;
        cookie.set_sensitive(true);
        let mut headers = HeaderMap::new();
        headers.insert(COOKIE, cookie);
        headers.insert(REFERER, HeaderValue::from_static("https://act.mihoyo.com/"));
        let data: RoleList = self.get_json(url, headers).await?.into_data()?;
        Ok(pick_official_role(data.list))
    }
}

/// 只取官服的角色，多个时取等级最高的。昵称和 UID 是外部文本，先清理。
fn pick_official_role(list: Vec<RoleItem>) -> Option<GameRole> {
    list.into_iter()
        .filter(|role| role.region == OFFICIAL_REGION)
        .max_by_key(|role| role.level)
        .map(|role| GameRole {
            nickname: printable(&role.nickname),
            level: role.level,
            game_uid: printable(&role.game_uid),
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use wiremock::matchers::{header, method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn ok(data: serde_json::Value) -> ResponseTemplate {
        ResponseTemplate::new(200)
            .set_body_json(json!({"retcode": 0, "message": "OK", "data": data}))
    }

    fn token() -> SecretString {
        SecretString::from("ct_secret")
    }

    #[tokio::test]
    async fn picks_the_highest_official_role() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(ROLES_PATH))
            .and(query_param("game_biz", "hk4e_cn"))
            .and(header(
                "cookie",
                "account_id=300012345; cookie_token=ct_secret",
            ))
            .respond_with(ok(json!({"list": [
                {"game_biz": "hk4e_cn", "region": "cn_qd01", "game_uid": "500000001", "nickname": "B服", "level": 60},
                {"game_biz": "hk4e_cn", "region": "cn_gf01", "game_uid": "100000002", "nickname": "小号", "level": 20},
                {"game_biz": "hk4e_cn", "region": "cn_gf01", "game_uid": "100000001", "nickname": "旅行者\u{202E}", "level": 58},
            ]})))
            .expect(1)
            .mount(&server)
            .await;

        let client = ApiClient::for_tests(&server.uri());
        let role = client.genshin_role("300012345", &token()).await.unwrap();
        assert_eq!(
            role,
            Some(GameRole {
                nickname: "旅行者".to_owned(),
                level: 58,
                game_uid: "100000001".to_owned(),
            })
        );
    }

    #[tokio::test]
    async fn no_official_role_is_none() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(ROLES_PATH))
            .respond_with(ok(json!({"list": [
                {"game_biz": "hk4e_cn", "region": "cn_qd01", "game_uid": "500000001", "nickname": "B服", "level": 60},
            ]})))
            .mount(&server)
            .await;
        let client = ApiClient::for_tests(&server.uri());
        assert_eq!(
            client.genshin_role("300012345", &token()).await.unwrap(),
            None
        );
    }

    #[tokio::test]
    async fn errors_keep_the_retcode_but_not_the_cookie_token() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(ROLES_PATH))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({"retcode": -100, "message": "登录失效", "data": null})),
            )
            .mount(&server)
            .await;
        let client = ApiClient::for_tests(&server.uri());
        let error = client
            .genshin_role("300012345", &token())
            .await
            .unwrap_err();
        let text = format!("{error} {error:?}");
        assert_eq!(error.retcode(), Some(-100), "{text}");
        assert!(!text.contains("ct_secret"), "{text}");
    }
}
