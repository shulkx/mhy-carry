//! account：查看已绑定的账号，或者解绑。

use std::fs;

use mhy_carry_core::account::AccountFile;
use mhy_carry_core::store::AccountStore;

use crate::{CliResult, context, prompt};

pub fn run(remove: bool) -> CliResult {
    let store = context::open_store()?;
    let file = store.load()?;
    // 账号文件损坏时留下的备份等旧文件，里面可能有明文凭证。
    let leftovers = store.leftover_files()?;

    match file.accounts.first() {
        Some(account) => {
            println!("账号：{}", account.masked_uid());
            println!("绑定时间：{}", account.bound_at);
            println!("凭证文件（明文）：{}", store.path().display());
        }
        None => println!("还没有绑定账号。运行 mhy-carry-cli bind 绑定。"),
    }
    if !leftovers.is_empty() {
        println!(
            "另有 {} 个旧的账号文件，里面可能有明文凭证：",
            leftovers.len()
        );
        for path in &leftovers {
            println!("  {}", path.display());
        }
    }

    if !remove || (file.accounts.is_empty() && leftovers.is_empty()) {
        return Ok(());
    }
    if !prompt::confirm("解绑会删除本机保存的凭证（包括上面列出的旧文件），确定吗？")?
    {
        println!("已取消。");
        return Ok(());
    }
    if !file.accounts.is_empty() {
        store.save(&AccountFile::empty())?;
    }
    for path in &leftovers {
        fs::remove_file(path).map_err(|e| format!("删除 {} 失败：{e}", path.display()))?;
    }
    println!("已删除本机凭证。");
    println!("这不会让凭证在服务器端失效；需要的话，请在米游社 App 里下线设备或修改密码。");
    Ok(())
}
