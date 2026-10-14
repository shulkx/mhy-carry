//! 直播间（设计文档第 6.5 节）：解析你输入的直播间网址或房间号。以后加抖音就加一种直播间。

use std::fmt;

use url::Url;

/// 一个直播间。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LiveRoom {
    /// B 站直播间。`id` 是你输入的房间号，可能是短号，取流时会换成真实房间号。
    Bilibili { id: u64 },
}

impl fmt::Display for LiveRoom {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LiveRoom::Bilibili { id } => write!(f, "B 站直播间 {id}"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RoomError {
    #[error("请输入直播间网址或房间号")]
    Empty,
    #[error("暂不支持短链接，请在浏览器里打开后，复制地址栏里的网址")]
    ShortLink,
    #[error(
        "认不出这个直播间，请输入 B 站直播间网址（比如 https://live.bilibili.com/12345）或房间号"
    )]
    Unrecognized,
}

/// 解析直播间网址或房间号。纯数字按 B 站房间号处理；网址可以省略 `https://`。
pub fn parse_room(input: &str) -> Result<LiveRoom, RoomError> {
    let input = input.trim();
    if input.is_empty() {
        return Err(RoomError::Empty);
    }
    if is_number(input) {
        return bilibili(input);
    }
    let url = if input.contains("://") {
        Url::parse(input)
    } else {
        Url::parse(&format!("https://{input}"))
    }
    .map_err(|_| RoomError::Unrecognized)?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(RoomError::Unrecognized);
    }
    match url.host_str() {
        Some("b23.tv") => Err(RoomError::ShortLink),
        // 路径里第一个纯数字的段就是房间号：/12345、/h5/12345、/blanc/12345。
        Some("live.bilibili.com") => url
            .path_segments()
            .into_iter()
            .flatten()
            .find(|segment| is_number(segment))
            .map_or(Err(RoomError::Unrecognized), bilibili),
        _ => Err(RoomError::Unrecognized),
    }
}

fn is_number(text: &str) -> bool {
    !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit())
}

fn bilibili(digits: &str) -> Result<LiveRoom, RoomError> {
    match digits.parse::<u64>() {
        Ok(id) if id > 0 => Ok(LiveRoom::Bilibili { id }),
        _ => Err(RoomError::Unrecognized),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROOM: LiveRoom = LiveRoom::Bilibili { id: 12345 };

    #[test]
    fn accepts_room_numbers() {
        assert_eq!(parse_room(" 12345 "), Ok(ROOM));
    }

    #[test]
    fn accepts_bilibili_urls() {
        for input in [
            "https://live.bilibili.com/12345",
            "http://live.bilibili.com/12345?spm_id_from=333.1007",
            "live.bilibili.com/12345",
            "https://live.bilibili.com/h5/12345",
            "https://live.bilibili.com/blanc/12345?liteVersion=true",
        ] {
            assert_eq!(parse_room(input), Ok(ROOM), "{input}");
        }
    }

    #[test]
    fn short_links_are_not_supported() {
        assert_eq!(parse_room("https://b23.tv/abcd"), Err(RoomError::ShortLink));
        assert_eq!(parse_room("b23.tv/abcd"), Err(RoomError::ShortLink));
    }

    #[test]
    fn rejects_everything_else() {
        assert_eq!(parse_room("   "), Err(RoomError::Empty));
        for input in [
            "0",
            "abc",
            "99999999999999999999999",
            "https://www.bilibili.com/video/BV1xx411c7mD",
            "https://live.bilibili.com/",
            "https://evil.cn/12345",
            "https://live.bilibili.com.evil.cn/12345",
            "ftp://live.bilibili.com/12345",
        ] {
            assert_eq!(parse_room(input), Err(RoomError::Unrecognized), "{input}");
        }
    }

    #[test]
    fn rooms_are_named_by_platform_and_number() {
        assert_eq!(ROOM.to_string(), "B 站直播间 12345");
    }
}
