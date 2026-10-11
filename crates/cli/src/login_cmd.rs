//! login：用已绑定的账号确认一个游戏登录二维码（设计文档第 6.2 节第 5–6 步）。

use std::error::Error;
use std::path::{Path, PathBuf};

use mhy_carry_api::Credentials;
use mhy_carry_core::qr::{LoginQr, parse_login_qr};
use mhy_carry_core::store::AccountStore;

use crate::context::Context;
use crate::{CliResult, prompt};

pub enum QrSource {
    Url(String),
    Image(PathBuf),
}

pub async fn run(source: QrSource) -> CliResult {
    let now = jiff::Timestamp::now().as_second();
    let (qr, from) = match &source {
        QrSource::Url(text) => (parse_login_qr(text, now)?, "命令行参数".to_owned()),
        QrSource::Image(path) => (
            pick_login_qr(&decode_image(path)?, now)?,
            format!("图片 {}", path.display()),
        ),
    };

    let ctx = Context::load()?;
    let file = ctx.store.load()?;
    let account = file
        .accounts
        .first()
        .ok_or("还没有绑定账号，先运行 mhy-carry-cli bind")?;

    let question = format!(
        "在{from}中发现{}登录二维码。用账号 {} 登录？确认后，显示这个二维码的设备将登录你的账号。",
        qr.game.display_name,
        account.masked_uid()
    );
    if !prompt::confirm(&question)? {
        println!("已取消。");
        return Ok(());
    }

    let credentials = Credentials {
        stoken: &account.stoken,
        mid: &account.mid,
    };
    println!("检查账号凭证……");
    ctx.api.check_stoken(&credentials).await?;
    println!("扫码……");
    let ticket = ctx.api.panda_scan(&qr).await?;
    let app = ctx.api.passport_scan(&ticket, &credentials).await?;
    if app.app_id != qr.game.passport_app_id {
        return Err(format!(
            "服务器说这个二维码是“{}”（{}）的登录码，不是{}，已取消登录",
            app.app_name, app.app_id, qr.game.display_name
        )
        .into());
    }
    println!("确认登录……");
    ctx.api.passport_confirm(&ticket, &credentials).await?;
    println!(
        "登录成功：显示这个二维码的{}已登录你的账号。",
        qr.game.display_name
    );
    Ok(())
}

/// 识别图片里的所有二维码，返回它们的文本。
fn decode_image(path: &Path) -> Result<Vec<String>, Box<dyn Error>> {
    let path_str = path.to_str().ok_or("图片路径里有无法识别的字符")?;
    let results = rxing::helpers::detect_multiple_in_file(path_str)
        .map_err(|e| format!("图片里没有识别出二维码：{e}"))?;
    Ok(results.iter().map(|r| r.getText().to_owned()).collect())
}

/// 取第一个合法的游戏登录二维码。
fn pick_login_qr(texts: &[String], now: i64) -> Result<LoginQr, String> {
    let mut last_error = None;
    for text in texts {
        match parse_login_qr(text, now) {
            Ok(qr) => return Ok(qr),
            Err(error) => last_error = Some(error),
        }
    }
    Err(match last_error {
        Some(error) => format!("图片里的二维码不是有效的游戏登录码：{error}"),
        None => "图片里没有识别出二维码".to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_760_000_000;
    const VALID: &str = "https://user.mihoyo.com/qr_code_in_game.html?app_id=4&biz_key=hk4e_cn&expire=4102444800&ticket=66cf3afb2a1c7a308832bd56";

    #[test]
    fn picks_the_first_valid_login_qr() {
        let texts = vec!["你好".to_owned(), VALID.to_owned()];
        let qr = pick_login_qr(&texts, NOW).unwrap();
        assert_eq!(qr.ticket, "66cf3afb2a1c7a308832bd56");
    }

    #[test]
    fn explains_why_no_qr_was_accepted() {
        let texts = vec!["https://evil.cn/".to_owned()];
        let error = pick_login_qr(&texts, NOW).unwrap_err();
        assert!(error.contains("不是有效的游戏登录码"), "{error}");
    }

    #[test]
    fn reports_when_nothing_was_decoded() {
        let error = pick_login_qr(&[], NOW).unwrap_err();
        assert!(error.contains("没有识别出二维码"), "{error}");
    }

    #[test]
    fn missing_image_file_is_an_error() {
        assert!(decode_image(Path::new("不存在的图片.png")).is_err());
    }
}
