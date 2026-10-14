//! 米哈游接口统一的返回结构 `{retcode, message, data}`。

use serde::Deserialize;

use crate::error::ApiError;

#[derive(Debug, Deserialize)]
pub struct Envelope<T> {
    retcode: i64,
    #[serde(default)]
    message: String,
    data: Option<T>,
}

impl<T> Envelope<T> {
    /// retcode 为 0 时取出 data；data 缺失也算错误。
    pub fn into_data(self) -> Result<T, ApiError> {
        self.check()?;
        self.data
            .ok_or_else(|| ApiError::Format("返回内容里没有 data".to_owned()))
    }

    /// 只要求 retcode 为 0，不关心 data。
    pub fn ensure_ok(self) -> Result<(), ApiError> {
        self.check()
    }

    /// retcode 为 0 时取出 data；data 缺失时返回 None。
    pub fn into_optional_data(self) -> Result<Option<T>, ApiError> {
        self.check()?;
        Ok(self.data)
    }

    fn check(&self) -> Result<(), ApiError> {
        if self.retcode == 0 {
            Ok(())
        } else {
            Err(ApiError::Retcode {
                code: self.retcode,
                message: self.message.clone(),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::de::IgnoredAny;

    fn parse(json: &str) -> Envelope<IgnoredAny> {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn non_zero_retcode_is_an_error() {
        let error = parse(r#"{"retcode":-3501,"message":"二维码已过期","data":null}"#)
            .into_data()
            .unwrap_err();
        assert_eq!(error.retcode(), Some(-3501));
    }

    #[test]
    fn missing_data_is_a_format_error() {
        let error = parse(r#"{"retcode":0,"message":"OK","data":null}"#)
            .into_data()
            .unwrap_err();
        assert!(matches!(error, ApiError::Format(_)));
    }

    #[test]
    fn optional_data_may_be_missing() {
        assert!(
            parse(r#"{"retcode":0,"message":"OK"}"#)
                .into_optional_data()
                .unwrap()
                .is_none()
        );
        let error = parse(r#"{"retcode":-100,"message":"登录失效"}"#)
            .into_optional_data()
            .unwrap_err();
        assert_eq!(error.retcode(), Some(-100));
    }

    #[test]
    fn ensure_ok_ignores_missing_data() {
        assert!(parse(r#"{"retcode":0,"message":"OK"}"#).ensure_ok().is_ok());
    }
}
