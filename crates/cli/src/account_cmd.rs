//! account：查看已绑定的账号，或者解绑。

use mhy_carry_core::account::AccountFile;
use mhy_carry_core::store::AccountStore;

use crate::{CliResult, context, prompt};

pub fn run(remove: bool) -> CliResult {
    let store = context::open_store()?;
    let file = store.load()?;
    let Some(account) = file.accounts.first() else {
        println!("还没有绑定账号。运行 mhy-carry-cli bind 绑定。");
        return Ok(());
    };
    println!("账号：{}", account.masked_uid());
    println!("绑定时间：{}", account.bound_at);
    println!("凭证文件（明文）：{}", store.path().display());

    if remove {
        if !prompt::confirm("解绑会删除本机保存的凭证，确定吗？")? {
            println!("已取消。");
            return Ok(());
        }
        store.save(&AccountFile::empty())?;
        println!("已删除本机凭证。");
        println!("这不会让凭证在服务器端失效；需要的话，请在米游社 App 里下线设备或修改密码。");
    }
    Ok(())
}
