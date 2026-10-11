//! 截图方案实验。用法：
//!   cargo run --release [--features wgc] -- <窗口标题关键字> <window|region> [秒数] [每秒帧数]

use std::error::Error;
use std::time::{Duration, Instant};

use xcap::image::RgbaImage;
use xcap::{Monitor, Window, XCapResult};

fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<String> = std::env::args().collect();
    let usage = "用法：capture-bench <窗口标题关键字> <window|region> [秒数] [每秒帧数]";
    let title = args.get(1).ok_or(usage)?;
    let method = args.get(2).map(String::as_str).ok_or(usage)?;
    let seconds: u64 = args.get(3).map(|s| s.parse()).transpose()?.unwrap_or(60);
    let fps: u64 = args.get(4).map(|s| s.parse()).transpose()?.unwrap_or(4);
    let backend = if cfg!(feature = "wgc") { "wgc" } else { "gdi" };

    let window = Window::all()?
        .into_iter()
        .find(|w| {
            w.title().map(|t| t.contains(title.as_str())).unwrap_or(false)
                && !w.is_minimized().unwrap_or(true)
        })
        .ok_or("没找到标题包含该关键字、且没有最小化的窗口")?;
    let monitor = window.current_monitor()?;
    println!(
        "窗口：title={:?} pid={} app_name={:?} 位置=({}, {}) 大小={}x{}",
        window.title()?,
        window.pid()?,
        window.app_name().unwrap_or_default(),
        window.x()?,
        window.y()?,
        window.width()?,
        window.height()?,
    );
    println!(
        "显示器：{:?} {}x{} 缩放={}",
        monitor.name()?,
        monitor.width()?,
        monitor.height()?,
        monitor.scale_factor()?,
    );
    println!("后端={backend} 截法={method} 持续 {seconds} 秒，每秒 {fps} 帧");

    let interval = Duration::from_millis(1000 / fps.max(1));
    let cpu_start = cpu_time::ProcessTime::now();
    let wall_start = Instant::now();
    let (mut frames, mut failures) = (0u32, 0u32);
    let mut capture_total = Duration::ZERO;

    while wall_start.elapsed() < Duration::from_secs(seconds) {
        let started = Instant::now();
        let result = match method {
            "window" => window.capture_image(),
            "region" => capture_window_region(&window, &monitor),
            _ => return Err("截法只能是 window 或 region".into()),
        };
        let took = started.elapsed();
        match result {
            Ok(image) => {
                if frames == 0 {
                    let file = format!("first-{backend}-{method}.png");
                    image.save(&file)?;
                    println!(
                        "第一帧 {}x{}，已保存到 {file}，非黑像素占比 {:.1}%",
                        image.width(),
                        image.height(),
                        non_black_ratio(&image) * 100.0
                    );
                }
                frames += 1;
                capture_total += took;
            }
            Err(e) => {
                failures += 1;
                eprintln!("截图失败：{e}");
            }
        }
        if let Some(rest) = interval.checked_sub(started.elapsed()) {
            std::thread::sleep(rest);
        }
    }

    let cpu = cpu_start.elapsed().as_secs_f64();
    let wall = wall_start.elapsed().as_secs_f64();
    let avg_ms = if frames > 0 {
        capture_total.as_secs_f64() * 1000.0 / f64::from(frames)
    } else {
        0.0
    };
    println!("成功 {frames} 帧，失败 {failures} 次，平均每帧截图 {avg_ms:.1} ms");
    println!("CPU 占用：一个核心的 {:.1}%", cpu / wall * 100.0);
    Ok(())
}

/// 截取窗口所在的屏幕区域（坐标相对于窗口所在的显示器）。
fn capture_window_region(window: &Window, monitor: &Monitor) -> XCapResult<RgbaImage> {
    let x = (window.x()? - monitor.x()?).max(0) as u32;
    let y = (window.y()? - monitor.y()?).max(0) as u32;
    let width = window.width()?.min(monitor.width()?.saturating_sub(x));
    let height = window.height()?.min(monitor.height()?.saturating_sub(y));
    monitor.capture_region(x, y, width, height)
}

/// 亮度高于很暗阈值的像素占比，用来判断是不是截到了黑屏。
fn non_black_ratio(image: &RgbaImage) -> f64 {
    let total = u64::from(image.width()) * u64::from(image.height());
    if total == 0 {
        return 0.0;
    }
    let bright = image
        .pixels()
        .filter(|p| p.0[0] > 16 || p.0[1] > 16 || p.0[2] > 16)
        .count() as f64;
    bright / total as f64
}
