//! login：用已绑定的账号确认一个游戏登录二维码（设计文档第 6.2 节第 5–6 步）。

use std::error::Error;
use std::path::{Path, PathBuf};

use mhy_carry_api::Credentials;
use mhy_carry_core::capture::Frame;
use mhy_carry_core::decode::{QrDecoder, RxingDecoder};
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
        qr.game.display_name, account.uid
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
    println!("登录中……");
    ctx.api.login_with_qr(&qr, &credentials).await?;
    println!(
        "登录成功：显示这个二维码的{}已登录你的账号。",
        qr.game.display_name
    );
    Ok(())
}

/// 识别图片里的所有二维码，返回它们的文本。图片先转成灰度，再交给 core 的识别器。
fn decode_image(path: &Path) -> Result<Vec<String>, Box<dyn Error>> {
    let luma = image::open(path)
        .map_err(|e| format!("读取图片 {} 失败：{e}", path.display()))?
        .into_luma8();
    let (width, height) = luma.dimensions();
    Ok(RxingDecoder.decode(Frame {
        width,
        height,
        luma: luma.into_raw(),
    }))
}

/// 取图片里唯一的游戏登录二维码。有不止一个不同的登录码时拒绝：没法判断该登录哪一个，
/// 截图里也可能混进别人故意放的码（设计文档第 9 节的二维码劫持）。
fn pick_login_qr(texts: &[String], now: i64) -> Result<LoginQr, String> {
    let mut found: Option<LoginQr> = None;
    let mut last_error = None;
    for text in texts {
        match parse_login_qr(text, now) {
            Ok(qr) => match &found {
                Some(first) if first.ticket != qr.ticket => {
                    return Err(
                        "图片里有不止一个游戏登录二维码，无法确定要登录哪一个。请只截取要登录的那个二维码"
                            .to_owned(),
                    );
                }
                Some(_) => {}
                None => found = Some(qr),
            },
            Err(error) => last_error = Some(error),
        }
    }
    found.ok_or_else(|| match last_error {
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
    fn refuses_several_different_login_qrs() {
        let other = VALID.replace("66cf3afb2a1c7a308832bd56", "77cf3afb2a1c7a308832bd57");
        let texts = vec![VALID.to_owned(), other];
        let error = pick_login_qr(&texts, NOW).unwrap_err();
        assert!(error.contains("不止一个"), "{error}");
    }

    #[test]
    fn the_same_qr_found_twice_is_fine() {
        let texts = vec![VALID.to_owned(), VALID.to_owned()];
        assert!(pick_login_qr(&texts, NOW).is_ok());
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

    /// 画一张真实的二维码 PNG，确认“读图片 → 转灰度 → 识别”整条路能走通。
    #[test]
    fn decodes_qr_from_png() {
        let code = qrcode::QrCode::new(VALID.as_bytes()).unwrap();
        let colors = code.to_colors();
        let modules = code.width() as u32;
        // 每个模块画成 4×4 像素，四周留 4 个模块宽的白边。
        let (scale, quiet) = (4, 4);
        let size = (modules + 2 * quiet) * scale;
        let img = image::GrayImage::from_fn(size, size, |x, y| {
            let (mx, my) = (x / scale, y / scale);
            let inside =
                (quiet..quiet + modules).contains(&mx) && (quiet..quiet + modules).contains(&my);
            let dark = inside
                && colors[((my - quiet) * modules + (mx - quiet)) as usize] == qrcode::Color::Dark;
            image::Luma([if dark { 0 } else { 255 }])
        });
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("qr.png");
        img.save(&path).unwrap();

        assert_eq!(decode_image(&path).unwrap(), vec![VALID.to_owned()]);
    }
}
