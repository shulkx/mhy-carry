//! B 站直播间取流和主播信息（设计文档第 6.5 节；核对来源见第 16 节）。不需要签名，也不带 Cookie。

use mhy_carry_core::text::clip;
use serde_json::Value;
use url::Url;

use crate::client::{LiveClient, LiveError};
use crate::platforms::{RoomInfo, StreamStatus};

/// 接口只能访问这个域名（设计文档第 9 节）。
const API_HOST: &str = "api.live.bilibili.com";
/// room_init 对不存在的房间返回的错误码（2026-10-15 实测）。其他错误码可能是临时的，会重试。
const ROOM_NOT_FOUND: i64 = 60004;
/// getH5InfoByRoom 对不存在的房间返回的错误码（2026-11-04 实测）。
const INFO_NOT_FOUND: i64 = 19002000;
/// 主播名、分区和标题最多保留多少个字（设计文档第 6.5 节）。
const MAX_ANCHOR: usize = 32;
const MAX_TEXT: usize = 64;
const REFERER: &str = "https://live.bilibili.com/";

pub(crate) struct Bilibili {
    base: Url,
}

impl Bilibili {
    pub fn official() -> Self {
        Self {
            base: Url::parse(&format!("https://{API_HOST}")).expect("固定的地址"),
        }
    }

    #[cfg(test)]
    pub fn at(base: &str) -> Self {
        Self {
            base: Url::parse(base).expect("测试服务器地址"),
        }
    }

    fn url(&self, path: &str, query: &[(&str, &str)]) -> Url {
        let mut url = self.base.clone();
        url.set_path(path);
        url.query_pairs_mut().extend_pairs(query);
        url
    }

    /// `room` 是你输入的房间号（可能是短号）。
    pub async fn resolve(&self, client: &LiveClient, room: u64) -> Result<StreamStatus, LiveError> {
        let room = room.to_string();
        let init = client
            .get_json(
                self.url("/room/v1/Room/room_init", &[("id", &room)]),
                &[API_HOST],
                REFERER,
            )
            .await?;
        match init["code"].as_i64() {
            Some(0) => {}
            Some(ROOM_NOT_FOUND) => return Err(LiveError::RoomNotFound),
            code => return Err(error_code(code)),
        }
        let real_id = init["data"]["room_id"]
            .as_u64()
            .ok_or_else(|| LiveError::Format("room_init 的返回里没有 room_id".to_owned()))?
            .to_string();
        if init["data"]["live_status"].as_i64() != Some(1) {
            return Ok(StreamStatus::Offline);
        }
        let info = client
            .get_json(
                self.url(
                    "/xlive/web-room/v2/index/getRoomPlayInfo",
                    &[
                        ("room_id", &real_id),
                        ("protocol", "0,1"),
                        ("format", "0,1,2"),
                        ("codec", "0"),
                        ("qn", "10000"),
                        ("platform", "web"),
                    ],
                ),
                &[API_HOST],
                REFERER,
            )
            .await?;
        if info["code"].as_i64() != Some(0) {
            return Err(LiveError::Format(format!(
                "getRoomPlayInfo 返回 code={}",
                info["code"]
            )));
        }
        // 两次请求之间主播可能刚好下播，这时没有 playurl_info。
        if info["data"]["playurl_info"].is_null() {
            return Ok(StreamStatus::Offline);
        }
        let urls = flv_urls(&info["data"]["playurl_info"]["playurl"]["stream"]);
        if urls.is_empty() {
            return Err(LiveError::NoStream);
        }
        Ok(StreamStatus::Live {
            urls,
            referer: REFERER,
        })
    }
}

impl Bilibili {
    /// 查主播名、是否在播、分区和标题（设计文档第 6.5 节“确认直播间”）。
    /// `room` 是你输入的房间号，短号也行。
    pub async fn lookup(&self, client: &LiveClient, room: u64) -> Result<RoomInfo, LiveError> {
        let room = room.to_string();
        let info = client
            .get_json(
                self.url(
                    "/xlive/web-room/v1/index/getH5InfoByRoom",
                    &[("room_id", &room)],
                ),
                &[API_HOST],
                REFERER,
            )
            .await?;
        match info["code"].as_i64() {
            Some(0) => {}
            Some(INFO_NOT_FOUND) => return Err(LiveError::RoomNotFound),
            code => return Err(error_code(code)),
        }
        let data = &info["data"];
        let anchor = clip(
            data["anchor_info"]["base_info"]["uname"]
                .as_str()
                .unwrap_or_default(),
            MAX_ANCHOR,
        );
        if anchor.is_empty() {
            return Err(LiveError::Format(
                "getH5InfoByRoom 的返回里没有主播名".to_owned(),
            ));
        }
        let text = |value: &Value| clip(value.as_str().unwrap_or_default(), MAX_TEXT);
        Ok(RoomInfo {
            anchor,
            live: data["room_info"]["live_status"].as_i64() == Some(1),
            area: text(&data["room_info"]["area_name"]),
            title: text(&data["room_info"]["title"]),
        })
    }
}

/// 接口返回的其他错误码：可能是风控之类的临时错误，可以重试。
fn error_code(code: Option<i64>) -> LiveError {
    LiveError::Other(format!(
        "B 站直播接口返回错误码 {}",
        code.map_or("（无）".to_owned(), |code| code.to_string())
    ))
}

/// 挑出 HTTP-FLV、H.264 的地址：`host + base_url + extra`，每个 CDN 一个。
fn flv_urls(streams: &Value) -> Vec<String> {
    let mut urls = Vec::new();
    for stream in streams.as_array().into_iter().flatten() {
        if stream["protocol_name"] != "http_stream" {
            continue;
        }
        for format in stream["format"].as_array().into_iter().flatten() {
            if format["format_name"] != "flv" {
                continue;
            }
            for codec in format["codec"].as_array().into_iter().flatten() {
                if codec["codec_name"] != "avc" {
                    continue;
                }
                let base = codec["base_url"].as_str().unwrap_or_default();
                for info in codec["url_info"].as_array().into_iter().flatten() {
                    if let (Some(host), Some(extra)) =
                        (info["host"].as_str(), info["extra"].as_str())
                    {
                        urls.push(format!("{host}{base}{extra}"));
                    }
                }
            }
        }
    }
    urls
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use serde_json::json;
    use wiremock::matchers::{header, method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    pub(crate) fn room_init(live: bool) -> ResponseTemplate {
        ResponseTemplate::new(200).set_body_json(json!({
            "code": 0,
            "data": {"room_id": 32155676, "short_id": 12345, "live_status": if live { 1 } else { 0 }}
        }))
    }

    /// `hosts` 是各 CDN 的地址前缀，每个都拼出一个 FLV 地址；另外放一个应该被忽略的 HLS 流。
    pub(crate) fn play_info(hosts: &[String]) -> ResponseTemplate {
        let url_info: Vec<_> = hosts
            .iter()
            .map(|host| json!({"host": host, "extra": "?sign=abc"}))
            .collect();
        ResponseTemplate::new(200).set_body_json(json!({
            "code": 0,
            "data": {"playurl_info": {"playurl": {"stream": [
                {"protocol_name": "http_stream", "format": [
                    {"format_name": "flv", "codec": [
                        {"codec_name": "avc", "current_qn": 10000, "base_url": "/live/stream.flv", "url_info": url_info}
                    ]}
                ]},
                {"protocol_name": "http_hls", "format": [
                    {"format_name": "ts", "codec": [
                        {"codec_name": "avc", "base_url": "/live/index.m3u8", "url_info": [{"host": "https://hls.example", "extra": ""}]}
                    ]}
                ]}
            ]}}}
        }))
    }

    pub(crate) async fn mount_room(server: &MockServer, live: bool, hosts: &[String]) {
        Mock::given(method("GET"))
            .and(path("/room/v1/Room/room_init"))
            .and(query_param("id", "12345"))
            .and(header("referer", REFERER))
            .respond_with(room_init(live))
            .mount(server)
            .await;
        Mock::given(method("GET"))
            .and(path("/xlive/web-room/v2/index/getRoomPlayInfo"))
            .and(query_param("room_id", "32155676"))
            .and(query_param("codec", "0"))
            .and(query_param("qn", "10000"))
            .respond_with(play_info(hosts))
            .mount(server)
            .await;
    }

    #[tokio::test]
    async fn live_rooms_give_one_flv_url_per_cdn() {
        let server = MockServer::start().await;
        let hosts = vec![
            "https://cdn-a.example".to_owned(),
            "https://cdn-b.example".to_owned(),
        ];
        mount_room(&server, true, &hosts).await;
        let status = Bilibili::at(&server.uri())
            .resolve(&LiveClient::for_tests(), 12345)
            .await
            .unwrap();
        assert_eq!(
            status,
            StreamStatus::Live {
                urls: vec![
                    "https://cdn-a.example/live/stream.flv?sign=abc".to_owned(),
                    "https://cdn-b.example/live/stream.flv?sign=abc".to_owned(),
                ],
                referer: REFERER,
            }
        );
    }

    #[tokio::test]
    async fn offline_rooms_do_not_ask_for_streams() {
        let server = MockServer::start().await;
        Mock::given(path("/room/v1/Room/room_init"))
            .respond_with(room_init(false))
            .mount(&server)
            .await;
        Mock::given(path("/xlive/web-room/v2/index/getRoomPlayInfo"))
            .respond_with(play_info(&[]))
            .expect(0)
            .mount(&server)
            .await;
        let status = Bilibili::at(&server.uri())
            .resolve(&LiveClient::for_tests(), 12345)
            .await
            .unwrap();
        assert_eq!(status, StreamStatus::Offline);
    }

    #[tokio::test]
    async fn unknown_rooms_are_reported() {
        let server = MockServer::start().await;
        Mock::given(path("/room/v1/Room/room_init"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({"code": 60004, "message": "直播间不存在", "data": null})),
            )
            .mount(&server)
            .await;
        let result = Bilibili::at(&server.uri())
            .resolve(&LiveClient::for_tests(), 12345)
            .await;
        assert!(matches!(result, Err(LiveError::RoomNotFound)), "{result:?}");
    }

    #[tokio::test]
    async fn other_error_codes_can_be_retried() {
        let server = MockServer::start().await;
        Mock::given(path("/room/v1/Room/room_init"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({"code": -412, "message": "请求被拦截", "data": null})),
            )
            .mount(&server)
            .await;
        let result = Bilibili::at(&server.uri())
            .resolve(&LiveClient::for_tests(), 12345)
            .await;
        assert!(
            matches!(&result, Err(LiveError::Other(message)) if message == "B 站直播接口返回错误码 -412"),
            "{result:?}"
        );
    }

    fn h5_info(body: serde_json::Value) -> ResponseTemplate {
        ResponseTemplate::new(200).set_body_json(body)
    }

    async fn lookup(server: &MockServer) -> Result<RoomInfo, LiveError> {
        Bilibili::at(&server.uri())
            .lookup(&LiveClient::for_tests(), 12345)
            .await
    }

    #[tokio::test]
    async fn looks_up_the_anchor_status_area_and_title() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/xlive/web-room/v1/index/getH5InfoByRoom"))
            .and(query_param("room_id", "12345"))
            .and(header("referer", REFERER))
            .respond_with(h5_info(json!({
                "code": 0,
                "data": {
                    "room_info": {
                        "room_id": 32155676,
                        "live_status": 1,
                        "area_name": "原神",
                        "title": format!("深渊{}速打", char::from(7)),
                    },
                    "anchor_info": {"base_info": {"uname": " 憨憨的小喵i "}}
                }
            })))
            .mount(&server)
            .await;
        assert_eq!(
            lookup(&server).await.unwrap(),
            RoomInfo {
                anchor: "憨憨的小喵i".to_owned(),
                live: true,
                area: "原神".to_owned(),
                title: "深渊 速打".to_owned(),
            }
        );
    }

    #[tokio::test]
    async fn long_texts_are_clipped_and_offline_rooms_say_so() {
        let server = MockServer::start().await;
        Mock::given(path("/xlive/web-room/v1/index/getH5InfoByRoom"))
            .respond_with(h5_info(json!({
                "code": 0,
                "data": {
                    "room_info": {"live_status": 2, "title": "长".repeat(100)},
                    "anchor_info": {"base_info": {"uname": "名".repeat(40)}}
                }
            })))
            .mount(&server)
            .await;
        let info = lookup(&server).await.unwrap();
        assert!(!info.live);
        assert_eq!(info.area, "");
        assert_eq!(info.anchor.chars().count(), 32);
        assert_eq!(info.title.chars().count(), 64);
        assert!(info.title.ends_with('…'));
    }

    #[tokio::test]
    async fn lookup_reports_missing_rooms_and_other_codes() {
        for (code, expected) in [
            (19002000, "找不到这个直播间"),
            (-352, "B 站直播接口返回错误码 -352"),
        ] {
            let server = MockServer::start().await;
            Mock::given(path("/xlive/web-room/v1/index/getH5InfoByRoom"))
                .respond_with(h5_info(json!({"code": code, "message": "x", "data": null})))
                .mount(&server)
                .await;
            let error = lookup(&server).await.unwrap_err();
            assert_eq!(error.to_string(), expected);
        }
    }

    #[tokio::test]
    async fn lookup_without_an_anchor_name_is_a_format_error() {
        let server = MockServer::start().await;
        Mock::given(path("/xlive/web-room/v1/index/getH5InfoByRoom"))
            .respond_with(h5_info(json!({"code": 0, "data": {"room_info": {}}})))
            .mount(&server)
            .await;
        assert!(matches!(lookup(&server).await, Err(LiveError::Format(_))));
    }

    #[tokio::test]
    async fn rooms_without_flv_are_reported() {
        let server = MockServer::start().await;
        mount_room(&server, true, &[]).await;
        let result = Bilibili::at(&server.uri())
            .resolve(&LiveClient::for_tests(), 12345)
            .await;
        assert!(matches!(result, Err(LiveError::NoStream)), "{result:?}");
    }
}
