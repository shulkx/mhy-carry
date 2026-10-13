//! bind：用米游社 App 扫码绑定账号。

use std::time::Duration;

use mhy_carry_api::{ApiError, BindStatus, retcode};
use mhy_carry_core::account::Account;
use mhy_carry_core::store::AccountStore;
use qrcode::QrCode;
use qrcode::render::unicode::Dense1x2;

use crate::context::Context;
use crate::{CliResult, prompt};

const POLL_INTERVAL: Duration = Duration::from_secs(2);
/// 查询状态没有副作用，网络出错允许重试一次，连续两次就放弃。
const MAX_NETWORK_ERRORS_IN_A_ROW: u32 = 2;

pub async fn run() -> CliResult {
    let ctx = Context::load()?;
    let mut file = ctx.store.load()?;
    if let Some(existing) = file.accounts.first() {
        let question = format!("已经绑定了账号 {}，要换成新账号吗？", existing.uid);
        if !prompt::confirm(&question)? {
            println!("已取消。");
            return Ok(());
        }
    }

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
            Ok(BindStatus::Confirmed(bound)) => {
                let account =
                    Account::new(bound.uid, bound.mid, bound.stoken, jiff::Timestamp::now());
                let uid = account.uid.clone();
                file.accounts = vec![account];
                ctx.store.save(&file)?;
                println!("绑定成功：{uid}");
                println!("凭证已保存（明文）：{}", ctx.store.path().display());
                return Ok(());
            }
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
