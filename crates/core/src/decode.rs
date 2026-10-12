//! 二维码识别（设计文档第 5 节）。v1 用 rxing，只开了它的二维码功能（见根目录 Cargo.toml）。

use std::panic;

use crate::capture::Frame;

/// 从一帧里识别出所有二维码的文本。
pub trait QrDecoder {
    fn decode(&self, frame: Frame) -> Vec<String>;
}

#[derive(Debug, Default, Clone, Copy)]
pub struct RxingDecoder;

impl QrDecoder for RxingDecoder {
    fn decode(&self, frame: Frame) -> Vec<String> {
        let Frame {
            width,
            height,
            luma,
        } = frame;
        if width == 0 || height == 0 || luma.len() != width as usize * height as usize {
            return Vec::new();
        }
        // rxing 是从 Java 移植的，遇到奇怪的画面可能 panic。一帧识别失败不能让整个扫描停下。
        let result = panic::catch_unwind(move || {
            rxing::helpers::detect_multiple_in_luma(luma, width, height)
        });
        match result {
            Ok(Ok(found)) => found.iter().map(|r| r.getText().to_owned()).collect(),
            // 画面里没有二维码时 rxing 也返回错误，当作“没识别出来”。
            Ok(Err(_)) | Err(_) => Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOGIN_URL: &str = "https://user.mihoyo.com/qr_code_in_game.html?app_id=4&biz_key=hk4e_cn&expire=4102444800&ticket=66cf3afb2a1c7a308832bd56";

    /// 带纹理的灰色背景，模拟游戏画面。
    fn background(width: u32, height: u32) -> Frame {
        let luma = (0..height)
            .flat_map(|y| (0..width).map(move |x| ((x * 7 + y * 13) % 64 + 96) as u8))
            .collect();
        Frame {
            width,
            height,
            luma,
        }
    }

    /// 把二维码画到 (left, top)：每个模块 4×4 像素，四周留 4 个模块宽的白边。
    fn draw_qr(frame: &mut Frame, text: &str, left: u32, top: u32) {
        let code = qrcode::QrCode::new(text.as_bytes()).unwrap();
        let colors = code.to_colors();
        let modules = code.width() as u32;
        let (scale, quiet) = (4, 4);
        let size = (modules + 2 * quiet) * scale;
        for y in 0..size {
            for x in 0..size {
                let (mx, my) = (x / scale, y / scale);
                let inside = (quiet..quiet + modules).contains(&mx)
                    && (quiet..quiet + modules).contains(&my);
                let dark = inside
                    && colors[((my - quiet) * modules + (mx - quiet)) as usize]
                        == qrcode::Color::Dark;
                let index = ((top + y) * frame.width + left + x) as usize;
                frame.luma[index] = if dark { 0 } else { 255 };
            }
        }
    }

    #[test]
    fn finds_a_qr_on_a_textured_background() {
        let mut frame = background(640, 480);
        draw_qr(&mut frame, LOGIN_URL, 200, 100);
        assert_eq!(RxingDecoder.decode(frame), vec![LOGIN_URL.to_owned()]);
    }

    #[test]
    fn finds_every_qr_in_the_frame() {
        let other = LOGIN_URL.replace("66cf", "77cf");
        let mut frame = background(800, 400);
        draw_qr(&mut frame, LOGIN_URL, 20, 40);
        draw_qr(&mut frame, &other, 440, 40);
        let mut texts = RxingDecoder.decode(frame);
        texts.sort();
        let mut expected = vec![LOGIN_URL.to_owned(), other];
        expected.sort();
        assert_eq!(texts, expected);
    }

    #[test]
    fn frame_without_qr_gives_nothing() {
        assert!(RxingDecoder.decode(background(320, 240)).is_empty());
    }

    #[test]
    fn malformed_frame_gives_nothing() {
        let frame = Frame {
            width: 10,
            height: 10,
            luma: vec![0; 5],
        };
        assert!(RxingDecoder.decode(frame).is_empty());
    }
}
