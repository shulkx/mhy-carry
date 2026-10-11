//! 各个命令共用的准备工作。

use std::error::Error;

use mhy_carry_api::ApiClient;
use mhy_carry_core::settings::Settings;
use mhy_carry_core::store::JsonFileStore;
use mhy_carry_platform::paths;

pub struct Context {
    pub store: JsonFileStore,
    pub api: ApiClient,
}

impl Context {
    /// 读取设置（首次运行时生成设备 ID）、打开账号文件、建好接口客户端。
    pub fn load() -> Result<Self, Box<dyn Error>> {
        let settings = Settings::load_or_init(&paths::settings_path()?)?;
        Ok(Self {
            store: open_store()?,
            api: ApiClient::new(settings.device_id)?,
        })
    }
}

/// 只需要读写账号文件、不需要联网时用。
pub fn open_store() -> Result<JsonFileStore, Box<dyn Error>> {
    Ok(JsonFileStore::new(paths::accounts_path()?))
}
