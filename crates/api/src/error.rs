//! 接口错误。错误信息里不会出现 URL 和凭证。

/// 已知的 retcode。
pub mod retcode {
    /// 登录状态失效（stoken 过期或被下线）。
    pub const TOKEN_INVALID: i64 = -100;
    /// 二维码已过期。
    pub const QR_EXPIRED: i64 = -3501;
    /// 用户在手机上取消了。
    pub const QR_CANCELLED: i64 = -3505;
}

#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error("网络请求失败：{0}")]
    Network(reqwest::Error),
    #[error("服务器返回 HTTP {0}")]
    Status(u16),
    #[error("{}", describe_retcode(.code, .message))]
    Retcode { code: i64, message: String },
    #[error("返回内容格式不对，接口可能已变更：{0}")]
    Format(String),
    #[error("请求被拦截：{0}")]
    Blocked(String),
}

impl ApiError {
    /// reqwest 的错误信息会带上完整 URL，统一先去掉。
    pub(crate) fn network(error: reqwest::Error) -> Self {
        Self::Network(error.without_url())
    }

    pub fn retcode(&self) -> Option<i64> {
        match self {
            Self::Retcode { code, .. } => Some(*code),
            _ => None,
        }
    }
}

fn describe_retcode(code: &i64, message: &str) -> String {
    match *code {
        retcode::TOKEN_INVALID => format!("登录状态已失效，请重新绑定账号（retcode={code}）"),
        retcode::QR_EXPIRED => format!("二维码已过期，请刷新后重试（retcode={code}）"),
        retcode::QR_CANCELLED => format!("已在手机上取消（retcode={code}）"),
        _ => format!("服务器返回错误 retcode={code}：{message}。接口可能已变更，请提 Issue"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_retcodes_have_friendly_messages() {
        let invalid = ApiError::Retcode {
            code: retcode::TOKEN_INVALID,
            message: "x".to_owned(),
        };
        assert!(invalid.to_string().contains("重新绑定"));
        let expired = ApiError::Retcode {
            code: retcode::QR_EXPIRED,
            message: "x".to_owned(),
        };
        assert!(expired.to_string().contains("过期"));
    }

    #[test]
    fn unknown_retcodes_show_the_server_message() {
        let error = ApiError::Retcode {
            code: -9999,
            message: "奇怪的错误".to_owned(),
        };
        let text = error.to_string();
        assert!(
            text.contains("-9999") && text.contains("奇怪的错误"),
            "{text}"
        );
        assert_eq!(error.retcode(), Some(-9999));
    }
}
