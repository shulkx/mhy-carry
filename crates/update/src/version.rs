//! 版本号：只认 `主.次.修订` 三段数字（设计文档第 6.6 节“检查”）。

use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Version {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
}

impl Version {
    /// 解析 `0.4.1` 这样的三段数字。前面带 `v`、预发布版本（`0.4.1-beta`）、多余的前导零都不认。
    pub fn parse(text: &str) -> Option<Self> {
        let mut parts = text.split('.').map(|part| {
            let digits = !part.is_empty()
                && part.len() <= 9
                && part.bytes().all(|byte| byte.is_ascii_digit());
            let leading_zero = part.len() > 1 && part.starts_with('0');
            if digits && !leading_zero {
                part.parse().ok()
            } else {
                None
            }
        });
        let version = Self {
            major: parts.next()??,
            minor: parts.next()??,
            patch: parts.next()??,
        };
        parts.next().is_none().then_some(version)
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_three_numbers() {
        assert_eq!(
            Version::parse("0.4.1"),
            Some(Version {
                major: 0,
                minor: 4,
                patch: 1
            })
        );
        assert_eq!(Version::parse("10.0.12").unwrap().to_string(), "10.0.12");
    }

    #[test]
    fn rejects_other_formats() {
        for text in [
            "",
            "0.4",
            "0.4.1.2",
            "v0.4.1",
            "0.4.1-beta",
            "0.4.x",
            "01.4.1",
            "0.4.",
            " 0.4.1",
            "9999999999.0.0",
        ] {
            assert_eq!(Version::parse(text), None, "{text}");
        }
    }

    #[test]
    fn compares_by_number() {
        let parse = |text| Version::parse(text).unwrap();
        assert!(parse("0.10.0") > parse("0.9.9"));
        assert!(parse("1.0.0") > parse("0.99.99"));
        assert!(parse("0.4.1") > parse("0.4.0"));
        assert_eq!(parse("0.4.1"), parse("0.4.1"));
    }
}
