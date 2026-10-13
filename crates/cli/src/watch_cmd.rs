//! watch：自动截图识别登录二维码，发现后问你要不要登录（设计文档第 6.2、6.3 节）。

use std::error::Error;
use std::io::{self, BufRead, Write};
use std::sync::Mutex;

use mhy_carry_core::capture::FrameSource;
use mhy_carry_core::decode::RxingDecoder;
use mhy_carry_core::games::GAMES;
use mhy_carry_core::scan::{Notice, Prompt, ScanStatus};
use mhy_carry_core::settings::TargetKind;
use mhy_carry_core::store::AccountStore;
use mhy_carry_platform::capture::{
    GameWindowSource, MonitorSource, PickedWindowSource, list_monitors, list_windows,
};
use mhy_carry_runner::{ScanJob, ScanUi, run_scan};
use tokio::sync::mpsc;

use crate::context::Context;
use crate::{CliResult, prompt};

/// 命令行里的截图目标。
#[derive(Debug, Clone, Copy, clap::ValueEnum)]
pub enum Target {
    /// 自动找游戏窗口
    Auto,
    /// 从列表里选一个窗口，比如正在看直播的浏览器
    Window,
    /// 整个显示器
    Monitor,
}

impl From<Target> for TargetKind {
    fn from(target: Target) -> Self {
        match target {
            Target::Auto => TargetKind::Auto,
            Target::Window => TargetKind::Window,
            Target::Monitor => TargetKind::Monitor,
        }
    }
}

pub async fn run(target: Option<TargetKind>) -> CliResult {
    let ctx = Context::load()?;
    let account = ctx
        .store
        .load()?
        .accounts
        .into_iter()
        .next()
        .ok_or("还没有绑定账号，先运行 mhy-carry-cli bind")?;
    let source = choose_source(target.unwrap_or(ctx.settings.scan.default_target))?;
    let job = ScanJob {
        source,
        decoder: Box::new(RxingDecoder),
        uid: account.uid,
        mid: account.mid,
        stoken: account.stoken,
        idle_timeout: ctx.settings.scan.idle_timeout(),
    };
    println!("开始扫描，按 Ctrl+C 结束。");
    // 命令行没有“停止”按钮，Ctrl+C 直接结束程序。
    let (_stop, stop_signal) = mpsc::channel(1);
    run_scan(&ctx.api, job, &TerminalUi::default(), stop_signal).await;
    Ok(())
}

/// 在终端里显示状态和提问。
#[derive(Default)]
struct TerminalUi {
    printer: Mutex<Printer>,
}

impl ScanUi for TerminalUi {
    fn show(&self, status: &ScanStatus) {
        self.printer
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .show(status);
    }

    async fn ask(&self, request: Prompt) -> bool {
        // 先响一声提示音，因为终端可能被游戏挡住。读不到终端输入时当作“否”。
        let question = format!("\x07{}", request.question());
        tokio::task::spawn_blocking(move || prompt::confirm(&question))
            .await
            .ok()
            .and_then(Result::ok)
            .unwrap_or(false)
    }
}

/// 按目标建好画面来源。选窗口、选显示器时在终端里列出来让你选。
fn choose_source(target: TargetKind) -> Result<Box<dyn FrameSource + Send>, Box<dyn Error>> {
    Ok(match target {
        TargetKind::Auto => Box::new(GameWindowSource::new(GAMES)),
        TargetKind::Window => {
            let windows = list_windows()?;
            if windows.is_empty() {
                return Err("没有可以选择的窗口".into());
            }
            // 游戏窗口排在最前面。
            let game = windows.iter().position(|window| window.is_game);
            if game.is_some() {
                println!("选择要扫描的窗口（直接回车选游戏窗口）：");
            } else {
                println!("选择要扫描的窗口：");
            }
            for (i, window) in windows.iter().enumerate() {
                println!("  {}. {}", i + 1, window.label);
            }
            let index = read_choice(windows.len(), game)?;
            Box::new(PickedWindowSource::new(&windows[index]))
        }
        TargetKind::Monitor => {
            let monitors = list_monitors()?;
            let primary = monitors.iter().position(|monitor| monitor.is_primary);
            let index = match monitors.len() {
                0 => return Err("没有找到显示器".into()),
                1 => 0,
                count => {
                    println!("选择要扫描的显示器（直接回车选主显示器）：");
                    for (i, monitor) in monitors.iter().enumerate() {
                        let mark = if monitor.is_primary {
                            "（主显示器）"
                        } else {
                            ""
                        };
                        println!("  {}. {}{mark}", i + 1, monitor.label);
                    }
                    read_choice(count, primary)?
                }
            };
            Box::new(MonitorSource::new(&monitors[index]))
        }
    })
}

/// 读一个 1 到 `count` 之间的序号，返回从 0 开始的下标。直接回车时用 `default`。
fn read_choice(count: usize, default: Option<usize>) -> io::Result<usize> {
    loop {
        print!("序号：");
        io::stdout().flush()?;
        let mut line = String::new();
        if io::stdin().lock().read_line(&mut line)? == 0 {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "没有输入"));
        }
        match parse_choice(&line, count, default) {
            Some(index) => return Ok(index),
            None => println!("请输入 1 到 {count} 之间的数字。"),
        }
    }
}

fn parse_choice(input: &str, count: usize, default: Option<usize>) -> Option<usize> {
    let input = input.trim();
    if input.is_empty() {
        return default;
    }
    match input.parse::<usize>() {
        Ok(number) if (1..=count).contains(&number) => Some(number - 1),
        _ => None,
    }
}

/// 状态有变化时打印一行，带时间；出现新的提示也打印一行。
#[derive(Default)]
struct Printer {
    last_line: Option<String>,
    last_notice: Option<Notice>,
}

impl Printer {
    fn show(&mut self, status: &ScanStatus) {
        match status {
            // 提问本身会打印出来；新的一次询问之后，同样的提示要能再打印一次。
            ScanStatus::AwaitingConfirm { .. } => {
                self.last_notice = None;
                return;
            }
            ScanStatus::LoggingIn => self.last_notice = None,
            _ => {}
        }
        let line = status.to_string();
        if self.last_line.as_ref() != Some(&line) {
            println!("[{}] {line}", now_hms());
            self.last_line = Some(line);
        }
        if let Some(notice) = status.notice()
            && self.last_notice.as_ref() != Some(notice)
        {
            println!("[{}] 提示：{notice}", now_hms());
            self.last_notice = Some(notice.clone());
        }
    }
}

fn now_hms() -> String {
    jiff::Zoned::now().strftime("%H:%M:%S").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn choice_numbers_start_at_one() {
        assert_eq!(parse_choice("1", 3, None), Some(0));
        assert_eq!(parse_choice(" 3\r\n", 3, None), Some(2));
    }

    #[test]
    fn out_of_range_or_garbage_is_rejected() {
        for input in ["0", "4", "-1", "abc"] {
            assert_eq!(parse_choice(input, 3, None), None, "{input}");
        }
    }

    #[test]
    fn empty_input_uses_the_default() {
        assert_eq!(parse_choice("\n", 3, Some(1)), Some(1));
        assert_eq!(parse_choice("", 3, None), None);
    }
}
