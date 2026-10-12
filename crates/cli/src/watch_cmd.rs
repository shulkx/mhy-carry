//! watch：自动截图识别登录二维码，发现后问你要不要登录（设计文档第 6.2、6.3 节）。

use std::collections::VecDeque;
use std::error::Error;
use std::io::{self, BufRead, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use mhy_carry_api::{ApiClient, ApiError, Credentials};
use mhy_carry_core::capture::FrameSource;
use mhy_carry_core::confirm::AlwaysAsk;
use mhy_carry_core::decode::RxingDecoder;
use mhy_carry_core::games::GAMES;
use mhy_carry_core::scan::{
    Action, Event, Failure, Notice, Now, Observation, Prompt, ScanSession, ScanStatus, observe,
};
use mhy_carry_core::settings::TargetKind;
use mhy_carry_core::store::AccountStore;
use mhy_carry_platform::capture::{
    GameWindowSource, MonitorSource, PickedWindowSource, list_monitors, list_windows,
};
use tokio::sync::mpsc;

use crate::context::Context;
use crate::{CliResult, prompt};

/// 每秒最多 4 帧（设计文档第 6.2 节）。
const FRAME_INTERVAL: Duration = Duration::from_millis(250);
/// 自动模式下找不到游戏窗口时，每 2 秒重找一次。
const SEARCH_INTERVAL: Duration = Duration::from_secs(2);
/// 这么久没有新画面时喂一次 Tick，让会话检查超时。
const TICK: Duration = Duration::from_secs(1);

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
    let file = ctx.store.load()?;
    let account = file
        .accounts
        .first()
        .ok_or("还没有绑定账号，先运行 mhy-carry-cli bind")?;
    let source = choose_source(target.unwrap_or(ctx.settings.scan.default_target))?;
    let credentials = Credentials {
        stoken: &account.stoken,
        mid: &account.mid,
    };

    let (mut session, first) = ScanSession::start(
        source.describe(),
        account.masked_uid(),
        ctx.settings.scan.idle_timeout(),
        Box::new(AlwaysAsk),
        Now::current(),
    );
    println!("开始扫描，按 Ctrl+C 结束。");
    let mut printer = Printer::default();
    printer.show(session.status());

    // 检查凭证、等你回答、登录期间不处理画面（设计文档第 6.2 节第 2 步），截图线程也跟着停下。
    let busy = Arc::new(AtomicBool::new(false));
    let mut frames = spawn_capture(source, Arc::clone(&busy));
    let mut actions = VecDeque::from([first]);
    loop {
        if !actions.is_empty() {
            busy.store(true, Ordering::Relaxed);
            while let Some(action) = actions.pop_front() {
                let event = match action {
                    Action::CheckCredential => {
                        Event::CredentialChecked(check_credential(&ctx.api, &credentials).await)
                    }
                    Action::Ask(request) => Event::Answered(ask(request).await?),
                    Action::Login(qr) => Event::LoginFinished(
                        ctx.api
                            .login_with_qr(&qr, &credentials)
                            .await
                            .map_err(Failure::from),
                    ),
                    Action::Finish => return Ok(()),
                };
                actions.extend(session.handle(event, Now::current()));
                printer.show(session.status());
            }
            busy.store(false, Ordering::Relaxed);
            // 动作之前截到、还留在通道里的画面已经过时，丢掉，只处理之后截到的。
            while frames.try_recv().is_ok() {}
        }
        let event = match tokio::time::timeout(TICK, frames.recv()).await {
            Ok(Some(observation)) => Event::Observed(observation),
            Ok(None) => return Err("截图线程意外退出".into()),
            Err(_) => Event::Tick,
        };
        actions.extend(session.handle(event, Now::current()));
        printer.show(session.status());
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
            println!("选择要扫描的窗口：");
            for (i, window) in windows.iter().enumerate() {
                println!("  {}. {}", i + 1, window.label);
            }
            let index = read_choice(windows.len(), None)?;
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

/// 在单独的线程里截图和识别（设计文档第 6.2 节第 2 步）。结果放进容量为 1 的通道，
/// 会话来不及处理时直接丢掉这一帧。`busy` 为真时不截图。主循环结束后通道关闭，线程跟着退出。
fn spawn_capture(
    mut source: Box<dyn FrameSource + Send>,
    busy: Arc<AtomicBool>,
) -> mpsc::Receiver<Observation> {
    let (sender, receiver) = mpsc::channel(1);
    std::thread::spawn(move || {
        while !sender.is_closed() {
            if busy.load(Ordering::Relaxed) {
                std::thread::sleep(FRAME_INTERVAL);
                continue;
            }
            let started = Instant::now();
            let observation = observe(source.as_mut(), &RxingDecoder);
            let interval = if observation == Observation::TargetMissing {
                SEARCH_INTERVAL
            } else {
                FRAME_INTERVAL
            };
            // 通道满了就丢掉这一帧；通道关了，下一轮循环会退出。
            let _ = sender.try_send(observation);
            if let Some(rest) = interval.checked_sub(started.elapsed()) {
                std::thread::sleep(rest);
            }
        }
    });
    receiver
}

/// 检查凭证。它没有副作用，网络出错时重试一次（设计文档第 10 节）。
async fn check_credential(api: &ApiClient, credentials: &Credentials<'_>) -> Result<(), Failure> {
    match api.check_stoken(credentials).await {
        Err(ApiError::Network(_)) => api.check_stoken(credentials).await.map_err(Failure::from),
        result => result.map_err(Failure::from),
    }
}

/// 在终端里问你要不要登录。先响一声提示音，因为终端可能被游戏挡住。
async fn ask(request: Prompt) -> io::Result<bool> {
    let question = format!("\x07{}", request.question());
    tokio::task::spawn_blocking(move || prompt::confirm(&question))
        .await
        .map_err(io::Error::other)?
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
