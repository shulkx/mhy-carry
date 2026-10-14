//! 各个命令共用的准备工作。

use std::error::Error;
use std::path::PathBuf;

use mhy_carry_api::ApiClient;
use mhy_carry_core::account::{Account, AccountFile};
use mhy_carry_core::settings::Settings;
use mhy_carry_core::store::JsonFileStore;
use mhy_carry_platform::paths;

pub struct Context {
    pub store: JsonFileStore,
    pub api: ApiClient,
    pub settings: Settings,
    pub settings_path: PathBuf,
}

impl Context {
    /// 读取设置（首次运行时生成设备 ID）、打开账号文件、建好接口客户端。建客户端不联网。
    pub fn load() -> Result<Self, Box<dyn Error>> {
        let settings_path = paths::settings_path()?;
        let settings = Settings::load_or_init(&settings_path)?;
        Ok(Self {
            store: JsonFileStore::new(paths::accounts_path()?),
            api: ApiClient::new(settings.device_id.clone())?,
            settings,
            settings_path,
        })
    }

    /// 要用的账号：指定了通行证 ID 就用它，否则用使用中的账号（设计文档第 6.4 节）。
    pub fn pick_account<'a>(
        &self,
        file: &'a AccountFile,
        uid: Option<&str>,
    ) -> Result<&'a Account, Box<dyn Error>> {
        match uid {
            Some(uid) => file
                .accounts
                .iter()
                .find(|account| account.uid == uid)
                .ok_or_else(|| {
                    format!("没有通行证 ID 为 {uid} 的账号，运行 mhy-carry-cli account 查看").into()
                }),
            None => file
                .active(self.settings.active_account.as_deref())
                .ok_or_else(|| "还没有绑定账号，先运行 mhy-carry-cli bind".into()),
        }
    }

    /// 改使用中的账号，并写进设置文件。None 表示改用列表里的第一个。
    pub fn set_active(&mut self, id: Option<String>) -> Result<(), Box<dyn Error>> {
        self.settings.active_account = id;
        self.settings.save(&self.settings_path)?;
        Ok(())
    }
}
