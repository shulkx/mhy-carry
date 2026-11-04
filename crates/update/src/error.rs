//! 检查更新和自动更新出错的原因。都是给人看的中文句子。

#[derive(Debug, thiserror::Error)]
pub enum UpdateError {
    #[error("网络请求失败：{0}")]
    Network(reqwest::Error),
    #[error("GitHub 返回 HTTP {0}")]
    Status(u16),
    #[error("请求被拦截：{0}")]
    Blocked(String),
    #[error("GitHub 返回的内容格式不对：{0}")]
    Format(String),
    #[error("文件太大，超过了 {} 的上限", size(*.0))]
    TooLarge(u64),
    /// 只在更新失败时显示，界面前面已经写了“更新失败：”。
    #[error("{0}")]
    Verify(String),
    #[error("写入更新文件失败：{0}。可以到发布页面手动下载。")]
    Io(#[from] std::io::Error),
}

impl UpdateError {
    /// reqwest 的错误信息带完整网址，下载地址后面有一长串签名参数，先去掉。
    /// 被跳转规则拦下的请求算作被拦截。
    pub(crate) fn network(error: reqwest::Error) -> Self {
        if error.is_redirect() {
            return Self::Blocked("下载地址跳到了 GitHub 以外的地方，或者跳转次数太多".to_owned());
        }
        Self::Network(error.without_url())
    }
}

fn size(bytes: u64) -> String {
    const MB: u64 = 1024 * 1024;
    if bytes >= MB {
        format!("{} MB", bytes / MB)
    } else {
        format!("{} KB", bytes.div_ceil(1024))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_are_readable() {
        assert!(
            UpdateError::TooLarge(64 * 1024 * 1024)
                .to_string()
                .contains("64 MB")
        );
        assert!(
            UpdateError::TooLarge(64 * 1024)
                .to_string()
                .contains("64 KB")
        );
    }
}
