//! 校验游戏客户端显示的登录二维码。所有条件都满足才算合法，否则忽略这个二维码。

use url::Url;

use crate::games::{self, GameProfile};

const HOST: &str = "user.mihoyo.com";
const PATH: &str = "/qr_code_in_game.html";
/// 允许本机时钟比服务器慢的秒数。
const CLOCK_SKEW_SECS: i64 = 60;
const PASSPORT_PATH: &str = "/login-platform/mobile.html";

/// 校验通过的游戏登录二维码。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoginQr {
    pub game: &'static GameProfile,
    pub ticket: String,
    /// 过期时间，Unix 秒。
    pub expire: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum QrError {
    #[error("不是网址")]
    NotUrl,
    #[error("不是 https 链接")]
    NotHttps,
    #[error("域名不是 user.mihoyo.com")]
    WrongHost,
    #[error("路径不对")]
    WrongPath,
    #[error("缺少参数 {0}")]
    MissingParam(&'static str),
    #[error("参数 {0} 出现了不止一次")]
    DuplicateParam(&'static str),
    #[error("参数 {0} 格式不对")]
    BadParam(&'static str),
    #[error("不支持的游戏（app_id={app_id}，biz_key={biz_key}）")]
    UnknownGame { app_id: String, biz_key: String },
    #[error("二维码已过期")]
    Expired,
}

/// 校验二维码文本。`now_unix` 是当前时间（Unix 秒）。
pub fn parse_login_qr(text: &str, now_unix: i64) -> Result<LoginQr, QrError> {
    let url = parse_mihoyo_url(text)?;
    if url.path() != PATH {
        return Err(QrError::WrongPath);
    }

    let app_id = single_param(&url, "app_id")?;
    let biz_key = single_param(&url, "biz_key")?;
    let ticket = single_param(&url, "ticket")?;
    let expire = single_param(&url, "expire")?;

    let app_id_num: u32 = app_id.parse().map_err(|_| QrError::BadParam("app_id"))?;
    let game = games::find(app_id_num, &biz_key).ok_or(QrError::UnknownGame { app_id, biz_key })?;

    let ticket_ok =
        (16..=64).contains(&ticket.len()) && ticket.bytes().all(|b| b.is_ascii_alphanumeric());
    if !ticket_ok {
        return Err(QrError::BadParam("ticket"));
    }

    let expire: i64 = expire.parse().map_err(|_| QrError::BadParam("expire"))?;
    if expire <= now_unix - CLOCK_SKEW_SECS {
        return Err(QrError::Expired);
    }

    Ok(LoginQr {
        game,
        ticket,
        expire,
    })
}

/// panda 扫码返回的 passport 二维码里的票据。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PassportTicket {
    pub tk: String,
    pub token_types: Vec<String>,
}

/// 校验 panda 扫码返回的 `passport_qr_url`，取出 `tk` 和 `token_types`。
pub fn parse_passport_url(text: &str) -> Result<PassportTicket, QrError> {
    let url = parse_mihoyo_url(text)?;
    if url.path() != PASSPORT_PATH {
        return Err(QrError::WrongPath);
    }
    let tk = single_param(&url, "tk")?;
    let types = single_param(&url, "token_types")?;

    let tk_ok =
        (8..=64).contains(&tk.len()) && tk.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-');
    if !tk_ok {
        return Err(QrError::BadParam("tk"));
    }

    let token_types: Vec<String> = types.split(',').map(str::to_owned).collect();
    let types_ok = token_types
        .iter()
        .all(|t| !t.is_empty() && t.bytes().all(|b| b.is_ascii_digit()));
    if !types_ok {
        return Err(QrError::BadParam("token_types"));
    }

    Ok(PassportTicket { tk, token_types })
}

/// 解析 URL，并检查是 https、host 正好是 user.mihoyo.com、没有 userinfo 和端口。
fn parse_mihoyo_url(text: &str) -> Result<Url, QrError> {
    let url = Url::parse(text.trim()).map_err(|_| QrError::NotUrl)?;
    if url.scheme() != "https" {
        return Err(QrError::NotHttps);
    }
    let has_userinfo = !url.username().is_empty() || url.password().is_some();
    if has_userinfo || url.port().is_some() || url.host_str() != Some(HOST) {
        return Err(QrError::WrongHost);
    }
    Ok(url)
}

/// 取一个只能出现一次的参数。
fn single_param(url: &Url, name: &'static str) -> Result<String, QrError> {
    let mut values = url
        .query_pairs()
        .filter(|(key, _)| key == name)
        .map(|(_, value)| value.into_owned());
    let first = values.next().ok_or(QrError::MissingParam(name))?;
    if values.next().is_some() {
        return Err(QrError::DuplicateParam(name));
    }
    Ok(first)
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_760_000_000;
    const TICKET: &str = "66cf3afb2a1c7a308832bd56";

    fn valid_query() -> String {
        format!(
            "app_id=4&app_name=%E5%8E%9F%E7%A5%9E&bbs=true&biz_key=hk4e_cn&expire=4102444800&ticket={TICKET}"
        )
    }

    fn url_with(query: &str) -> String {
        format!("https://user.mihoyo.com/qr_code_in_game.html?{query}")
    }

    fn query_without(name: &str) -> String {
        let prefix = format!("{name}=");
        valid_query()
            .split('&')
            .filter(|kv| !kv.starts_with(&prefix))
            .collect::<Vec<_>>()
            .join("&")
    }

    fn query_replacing(name: &str, value: &str) -> String {
        let prefix = format!("{name}=");
        valid_query()
            .split('&')
            .map(|kv| {
                if kv.starts_with(&prefix) {
                    format!("{name}={value}")
                } else {
                    kv.to_owned()
                }
            })
            .collect::<Vec<_>>()
            .join("&")
    }

    #[test]
    fn accepts_genshin_client_qr() {
        let qr = parse_login_qr(&url_with(&valid_query()), NOW).unwrap();
        assert_eq!(qr.game.biz_key, "hk4e_cn");
        assert_eq!(qr.ticket, TICKET);
        assert_eq!(qr.expire, 4_102_444_800);
    }

    #[test]
    fn ignores_surrounding_whitespace() {
        let text = format!("  {}\n", url_with(&valid_query()));
        assert!(parse_login_qr(&text, NOW).is_ok());
    }

    #[test]
    fn rejects_non_url() {
        assert_eq!(parse_login_qr("你好", NOW), Err(QrError::NotUrl));
    }

    #[test]
    fn rejects_http() {
        let text = format!(
            "http://user.mihoyo.com/qr_code_in_game.html?{}",
            valid_query()
        );
        assert_eq!(parse_login_qr(&text, NOW), Err(QrError::NotHttps));
    }

    #[test]
    fn rejects_lookalike_hosts() {
        for host in [
            "user.mihoyo.com.evil.cn",
            "evil.cn",
            "xuser.mihoyo.com",
            "sub.user.mihoyo.com",
            "user.mihoyo.com.",
            "user.mihoyo.com@evil.cn",
            "evil@user.mihoyo.com",
            "user.mihoyo.com:8443",
        ] {
            let text = format!("https://{host}/qr_code_in_game.html?{}", valid_query());
            assert_eq!(
                parse_login_qr(&text, NOW),
                Err(QrError::WrongHost),
                "{host}"
            );
        }
    }

    #[test]
    fn rejects_host_hidden_in_path() {
        let text = format!(
            "https://evil.cn/user.mihoyo.com/qr_code_in_game.html?{}",
            valid_query()
        );
        assert_eq!(parse_login_qr(&text, NOW), Err(QrError::WrongHost));
    }

    #[test]
    fn rejects_wrong_path() {
        for path in ["/qr_code_in_game.html/", "/login.html", "/"] {
            let text = format!("https://user.mihoyo.com{path}?{}", valid_query());
            assert_eq!(
                parse_login_qr(&text, NOW),
                Err(QrError::WrongPath),
                "{path}"
            );
        }
    }

    #[test]
    fn rejects_missing_params() {
        for name in ["app_id", "biz_key", "ticket", "expire"] {
            let text = url_with(&query_without(name));
            assert_eq!(
                parse_login_qr(&text, NOW),
                Err(QrError::MissingParam(name)),
                "{name}"
            );
        }
    }

    #[test]
    fn rejects_duplicate_params() {
        let text = url_with(&format!("{}&ticket={TICKET}", valid_query()));
        assert_eq!(
            parse_login_qr(&text, NOW),
            Err(QrError::DuplicateParam("ticket"))
        );
    }

    #[test]
    fn rejects_unknown_or_mismatched_game() {
        for (app_id, biz_key) in [("8", "hk4e_cn"), ("4", "nap_cn"), ("12", "nap_cn")] {
            let query = query_replacing("biz_key", biz_key);
            let query = query
                .split('&')
                .map(|kv| {
                    if kv.starts_with("app_id=") {
                        format!("app_id={app_id}")
                    } else {
                        kv.to_owned()
                    }
                })
                .collect::<Vec<_>>()
                .join("&");
            let result = parse_login_qr(&url_with(&query), NOW);
            assert!(
                matches!(result, Err(QrError::UnknownGame { .. })),
                "{app_id} {biz_key}: {result:?}"
            );
        }
    }

    #[test]
    fn rejects_non_numeric_app_id() {
        let text = url_with(&query_replacing("app_id", "abc"));
        assert_eq!(parse_login_qr(&text, NOW), Err(QrError::BadParam("app_id")));
    }

    #[test]
    fn rejects_bad_tickets() {
        let too_long = "a".repeat(65);
        for ticket in [
            "short",
            too_long.as_str(),
            "66cf3afb2a1c7a308832bd5!",
            "66cf3afb2a1c7a30%208832bd56",
        ] {
            let text = url_with(&query_replacing("ticket", ticket));
            assert_eq!(
                parse_login_qr(&text, NOW),
                Err(QrError::BadParam("ticket")),
                "{ticket}"
            );
        }
    }

    #[test]
    fn expiry_allows_60_seconds_of_clock_skew() {
        let almost = url_with(&query_replacing("expire", &(NOW - 59).to_string()));
        assert!(parse_login_qr(&almost, NOW).is_ok());

        let expired = url_with(&query_replacing("expire", &(NOW - 60).to_string()));
        assert_eq!(parse_login_qr(&expired, NOW), Err(QrError::Expired));

        let garbage = url_with(&query_replacing("expire", "abc"));
        assert_eq!(
            parse_login_qr(&garbage, NOW),
            Err(QrError::BadParam("expire"))
        );
    }
}

#[cfg(test)]
mod proptests {
    use super::*;
    use proptest::prelude::*;

    const NOW: i64 = 1_760_000_000;

    proptest! {
        #[test]
        fn never_panics_on_any_input(text in ".*") {
            let _ = parse_login_qr(&text, NOW);
        }

        #[test]
        fn any_change_to_the_host_is_rejected(
            prefix in "[a-z0-9.\\-]{0,10}",
            suffix in "[a-z0-9.\\-]{0,10}",
        ) {
            prop_assume!(!(prefix.is_empty() && suffix.is_empty()));
            let text = format!(
                "https://{prefix}user.mihoyo.com{suffix}/qr_code_in_game.html?app_id=4&biz_key=hk4e_cn&expire=4102444800&ticket=66cf3afb2a1c7a308832bd56"
            );
            prop_assert!(parse_login_qr(&text, NOW).is_err());
        }
    }
}

#[cfg(test)]
mod passport_tests {
    use super::*;

    const TK: &str = "4f1c2a9e-7b3d-4c55-9a1e-0d2b6f8c9e10";

    fn passport_url(query: &str) -> String {
        format!("https://user.mihoyo.com/login-platform/mobile.html?{query}#/login/qr")
    }

    #[test]
    fn accepts_passport_url_from_panda_scan() {
        let text = passport_url(&format!("expire=1760000300&tk={TK}&token_types=1"));
        assert_eq!(
            parse_passport_url(&text),
            Ok(PassportTicket {
                tk: TK.to_owned(),
                token_types: vec!["1".to_owned()],
            })
        );
    }

    #[test]
    fn splits_multiple_token_types() {
        let text = passport_url(&format!("tk={TK}&token_types=1,4"));
        assert_eq!(
            parse_passport_url(&text).unwrap().token_types,
            vec!["1", "4"]
        );
    }

    #[test]
    fn rejects_wrong_host_or_path() {
        let evil = format!("https://evil.cn/login-platform/mobile.html?tk={TK}&token_types=1");
        assert_eq!(parse_passport_url(&evil), Err(QrError::WrongHost));
        let game = format!("https://user.mihoyo.com/qr_code_in_game.html?tk={TK}&token_types=1");
        assert_eq!(parse_passport_url(&game), Err(QrError::WrongPath));
    }

    #[test]
    fn rejects_missing_params() {
        assert_eq!(
            parse_passport_url(&passport_url("token_types=1")),
            Err(QrError::MissingParam("tk"))
        );
        assert_eq!(
            parse_passport_url(&passport_url(&format!("tk={TK}"))),
            Err(QrError::MissingParam("token_types"))
        );
    }

    #[test]
    fn rejects_bad_tk() {
        for tk in ["short", "4f1c2a9e 7b3d", "4f1c2a9e_7b3d_4c55"] {
            let text = passport_url(&format!("tk={tk}&token_types=1"));
            assert_eq!(
                parse_passport_url(&text),
                Err(QrError::BadParam("tk")),
                "{tk}"
            );
        }
    }

    #[test]
    fn rejects_bad_token_types() {
        for types in ["", "a", "1,", "1;2"] {
            let text = passport_url(&format!("tk={TK}&token_types={types}"));
            assert_eq!(
                parse_passport_url(&text),
                Err(QrError::BadParam("token_types")),
                "{types}"
            );
        }
    }
}
