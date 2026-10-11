//! 各个命令共用的准备工作。

use std::error::Error;

use mhy_carry_core::store::JsonFileStore;
use mhy_carry_platform::paths;

/// 只需要读写账号文件、不需要联网时用。
pub fn open_store() -> Result<JsonFileStore, Box<dyn Error>> {
    Ok(JsonFileStore::new(paths::accounts_path()?))
}
