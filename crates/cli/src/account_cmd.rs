//! account：查看已绑定的账号；切换使用中的账号、写备注、解绑（设计文档第 6.4 节）。

use std::fs;

use mhy_carry_api::{Credentials, retcode};
use mhy_carry_core::account::{Account, normalize_note};
use mhy_carry_core::store::AccountStore;

use crate::context::Context;
use crate::{CliResult, prompt};

pub enum Action {
    List,
    Refresh,
    Use(String),
    Note { uid: String, note: String },
    Remove(String),
    RemoveLeftovers,
}

pub async fn run(action: Action) -> CliResult {
    let mut ctx = Context::load()?;
    match action {
        Action::List => list(&ctx),
        Action::Refresh => refresh(&ctx).await,
        Action::Use(uid) => use_account(&mut ctx, &uid),
        Action::Note { uid, note } => set_note(&ctx, &uid, &note),
        Action::Remove(uid) => remove(&mut ctx, &uid),
        Action::RemoveLeftovers => remove_leftovers(&ctx),
    }
}

fn list(ctx: &Context) -> CliResult {
    let file = ctx.store.load()?;
    if file.accounts.is_empty() {
        println!("还没有绑定账号。运行 mhy-carry-cli bind 绑定。");
    } else {
        let active = file
            .active(ctx.settings.active_account.as_deref())
            .map(|account| account.id.clone());
        println!("账号（* 是使用中的账号）：");
        for account in &file.accounts {
            let mark = if active.as_ref() == Some(&account.id) {
                "*"
            } else {
                " "
            };
            println!("{mark} {}", summary(account));
        }
        println!("凭证文件（明文）：{}", ctx.store.path().display());
    }
    let leftovers = ctx.store.leftover_files()?;
    if !leftovers.is_empty() {
        println!(
            "另有 {} 个旧的账号文件，里面可能有明文凭证（用 --remove-leftovers 删除）：",
            leftovers.len()
        );
        for path in &leftovers {
            println!("  {}", path.display());
        }
    }
    Ok(())
}

/// 一行说明一个账号：名称、通行证 ID、原神角色、绑定日期。
fn summary(account: &Account) -> String {
    let role = match &account.game_role {
        Some(role) => format!(
            "原神 {} Lv.{}（UID {}）",
            role.nickname, role.level, role.game_uid
        ),
        None => "原神角色未查到".to_owned(),
    };
    let date = account
        .bound_at
        .to_zoned(jiff::tz::TimeZone::system())
        .strftime("%Y-%m-%d");
    format!(
        "{}  通行证 {}  {role}  绑定于 {date}",
        account.display_name(),
        account.uid
    )
}

/// 逐个检查凭证、查原神角色。查角色的错误照实打印（retcode 和 message），方便核对接口
/// （设计文档第 14 节第 7 项）。
async fn refresh(ctx: &Context) -> CliResult {
    let mut file = ctx.store.load()?;
    if file.accounts.is_empty() {
        println!("还没有绑定账号。");
        return Ok(());
    }
    let mut changed = false;
    for account in &mut file.accounts {
        println!("{}（通行证 {}）", account.display_name(), account.uid);
        let credentials = Credentials {
            stoken: &account.stoken,
            mid: &account.mid,
        };
        let token = match ctx.api.check_stoken(&credentials).await {
            Ok(token) => {
                println!("  凭证有效");
                token
            }
            Err(error) if error.retcode() == Some(retcode::TOKEN_INVALID) => {
                println!("  凭证已失效，请重新绑定（mhy-carry-cli bind）");
                continue;
            }
            Err(error) => {
                println!("  检查凭证失败：{error}");
                continue;
            }
        };
        let Some(token) = token else {
            println!("  服务器没有返回 cookie_token，查不了原神角色");
            continue;
        };
        match ctx.api.genshin_role(&account.uid, &token).await {
            Ok(Some(role)) => {
                println!(
                    "  原神 {} Lv.{}（UID {}）",
                    role.nickname, role.level, role.game_uid
                );
                if account.game_role.as_ref() != Some(&role) {
                    account.game_role = Some(role);
                    changed = true;
                }
            }
            Ok(None) => println!("  这个通行证在原神官服没有角色"),
            Err(error) => println!("  查原神角色失败：{error}"),
        }
    }
    if changed {
        ctx.store.save(&file)?;
    }
    Ok(())
}

fn use_account(ctx: &mut Context, uid: &str) -> CliResult {
    let file = ctx.store.load()?;
    let account = ctx.pick_account(&file, Some(uid))?;
    let (id, name) = (account.id.clone(), account.display_name());
    ctx.set_active(Some(id))?;
    println!("使用中的账号改为 {name}。");
    Ok(())
}

fn set_note(ctx: &Context, uid: &str, note: &str) -> CliResult {
    let mut file = ctx.store.load()?;
    let id = ctx.pick_account(&file, Some(uid))?.id.clone();
    let account = file.get_mut(&id).expect("刚找到的账号");
    account.note = normalize_note(note);
    let name = account.display_name();
    ctx.store.save(&file)?;
    println!("已保存。这个账号现在显示为 {name}。");
    Ok(())
}

fn remove(ctx: &mut Context, uid: &str) -> CliResult {
    let mut file = ctx.store.load()?;
    let account = ctx.pick_account(&file, Some(uid))?;
    let (id, label) = (account.id.clone(), account.describe());
    if !prompt::confirm(&format!("解绑{label}？会删除本机保存的这个账号的凭证。"))?
    {
        println!("已取消。");
        return Ok(());
    }
    file.remove(&id);
    ctx.store.save(&file)?;
    if ctx.settings.active_account.as_deref() == Some(id.as_str()) {
        ctx.set_active(None)?;
    }
    println!("已解绑。");
    println!("这不会让凭证在服务器端失效；需要的话，请在米游社 App 里下线设备或修改密码。");
    Ok(())
}

fn remove_leftovers(ctx: &Context) -> CliResult {
    let leftovers = ctx.store.leftover_files()?;
    if leftovers.is_empty() {
        println!("没有旧的账号文件。");
        return Ok(());
    }
    for path in &leftovers {
        println!("  {}", path.display());
    }
    if !prompt::confirm(&format!(
        "删除上面 {} 个旧的账号文件？里面可能有明文凭证。",
        leftovers.len()
    ))? {
        println!("已取消。");
        return Ok(());
    }
    for path in &leftovers {
        fs::remove_file(path).map_err(|e| format!("删除 {} 失败：{e}", path.display()))?;
    }
    println!("已删除。");
    Ok(())
}
