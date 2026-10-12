//! 官方域名和接口地址。

use url::Url;

/// panda 扫码。各游戏通用，路径是 `/{biz_key}/combo/panda/qrcode/scan`。
/// M1 已用真实账号实测可用（设计文档第 14 节）。
pub const PANDA_HOST: &str = "api-sdk.mihoyo.com";
/// 绑定、检查凭证、passport 扫码和确认。
pub const PASSPORT_HOST: &str = "passport-api.mihoyo.com";

#[derive(Debug, Clone)]
pub struct Endpoints {
    panda: Url,
    passport: Url,
}

impl Endpoints {
    pub fn official() -> Self {
        Self {
            panda: https_base(PANDA_HOST),
            passport: https_base(PASSPORT_HOST),
        }
    }

    /// 测试时把所有接口都指到本地假服务器。
    #[cfg(test)]
    pub fn all_at(base: &str) -> Self {
        let base = Url::parse(base).expect("测试服务器地址");
        Self {
            panda: base.clone(),
            passport: base,
        }
    }

    pub fn panda_scan(&self, biz_key: &str) -> Url {
        at(&self.panda, &format!("/{biz_key}/combo/panda/qrcode/scan"))
    }

    pub fn passport(&self, path: &str) -> Url {
        at(&self.passport, path)
    }
}

fn https_base(host: &str) -> Url {
    Url::parse(&format!("https://{host}")).expect("固定的官方地址")
}

fn at(base: &Url, path: &str) -> Url {
    let mut url = base.clone();
    url.set_path(path);
    url
}
