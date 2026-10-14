//! bind：用米游社 App 扫码绑定账号。

use std::time::Duration;

use mhy_carry_api::{ApiClient, ApiError, BindStatus, BoundAccount, Credentials, retcode};
use mhy_carry_core::account::{Account, GameRole};
use mhy_carry_core::store::AccountStore;
use qrcode::QrCode;
use qrcode::render::unicode::Dense1x2;

use crate::CliResult;
use crate::context::Context;

const POLL_INTERVAL: Duration = Duration::from_secs(2);
/// 查询状态没有副作用，网络出错允许重试一次，连续两次就放弃。
const MAX_NETWORK_ERRORS_IN_A_ROW: u32 = 2;

pub async fn run() -> CliResult {
    let mut ctx = Context::load()?;
    // 先读一次：账号文件格式比本程序新、读写出错时，不用让你白扫一次码。
    ctx.store.load()?;

    let qr = ctx.api.create_bind_qr().await?;
    println!("请用米游社 App 扫描下面的二维码，并在手机上确认登录：");
    println!("（扫不出来的话，把终端换成深色背景再试）\n");
    println!("{}", render_qr(&qr.url)?);

    let mut announced_scanned = false;
    let mut network_errors = 0;
    loop {
        tokio::time::sleep(POLL_INTERVAL).await;
        match ctx.api.query_bind_qr(&qr.ticket).await {
            Ok(BindStatus::Created) => network_errors = 0,
            Ok(BindStatus::Scanned) => {
                network_errors = 0;
                if !announced_scanned {
                    println!("已扫码，请在手机上确认。");
                    announced_scanned = true;
                }
            }
            Ok(BindStatus::Confirmed(bound)) => return save(&mut ctx, bound).await,
            Err(ApiError::Network(_)) if network_errors + 1 < MAX_NETWORK_ERRORS_IN_A_ROW => {
                network_errors += 1;
            }
            Err(error) if error.retcode() == Some(retcode::QR_EXPIRED) => {
                return Err("二维码已过期，请重新运行 mhy-carry-cli bind".into());
            }
            Err(error) => return Err(error.into()),
        }
    }
}

/// 保存绑定的账号并设为使用中，再查一次原神角色（设计文档第 6.1、6.4 节）。
async fn save(ctx: &mut Context, bound: BoundAccount) -> CliResult {
    let mut file = ctx.store.load()?;
    let saved = file.upsert(Account::new(
        bound.uid,
        bound.mid,
        bound.stoken,
        jiff::Timestamp::now(),
    ));
    ctx.store.save(&file)?;
    ctx.set_active(Some(saved.id.clone()))?;

    let role = {
        let account = file.get(&saved.id).expect("刚保存的账号");
        if saved.added {
            println!("已添加账号：通行证 {}，并设为使用中。", account.uid);
        } else {
            println!(
                "已更新账号 {} 的凭证，并设为使用中。",
                account.display_name()
            );
        }
        println!("凭证已保存（明文）：{}", ctx.store.path().display());
        lookup_role(&ctx.api, account).await
    };
    match role {
        Ok(Some(role)) => {
            println!(
                "原神角色：{} Lv.{}（UID {}）",
                role.nickname, role.level, role.game_uid
            );
            file.get_mut(&saved.id).expect("刚保存的账号").game_role = Some(role);
            ctx.store.save(&file)?;
        }
        Ok(None) => println!("这个通行证在原神官服没有角色。"),
        Err(message) => println!("查原神角色失败（不影响使用）：{message}"),
    }
    Ok(())
}

/// 查原神角色。返回 Err 时是给人看的原因。
async fn lookup_role(api: &ApiClient, account: &Account) -> Result<Option<GameRole>, String> {
    let credentials = Credentials {
        stoken: &account.stoken,
        mid: &account.mid,
    };
    let token = api
        .check_stoken(&credentials)
        .await
        .map_err(|error| error.to_string())?
        .ok_or("服务器没有返回 cookie_token")?;
    api.genshin_role(&account.uid, &token)
        .await
        .map_err(|error| error.to_string())
}

/// 把二维码画成终端字符。按深色背景反色，白块是二维码的底色。
fn render_qr(content: &str) -> Result<String, qrcode::types::QrError> {
    let code = QrCode::new(content.as_bytes())?;
    Ok(code
        .render::<Dense1x2>()
        .dark_color(Dense1x2::Light)
        .light_color(Dense1x2::Dark)
        .build())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_qr_as_terminal_blocks() {
        let text = render_qr("https://user.mihoyo.com/login-platform/x").unwrap();
        assert!(text.lines().count() > 10, "{text}");
        assert!(text.contains('█') || text.contains('▀') || text.contains('▄'));
    }
}
