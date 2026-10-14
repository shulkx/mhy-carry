//! 扫码会话状态机（设计文档第 5、6.2、6.3 节）。
//!
//! 纯逻辑：不开线程，不做 I/O。运行循环把发生的事（[`Event`]）喂进来，它返回下一步
//! 要做的事（[`Action`]），并更新给界面显示的状态快照（[`ScanStatus`]）。

use std::collections::HashSet;
use std::fmt;
use std::time::{Duration, Instant};

use serde::Serialize;

use crate::capture::{CaptureError, FrameSource, SourceInfo, SourceKind};
use crate::confirm::{ConfirmPolicy, Decision, enforce_hard_rules};
use crate::decode::QrDecoder;
use crate::qr::{LoginQr, parse_login_qr};
use crate::text::printable;

/// 连续截图失败这么多次就停止（设计文档第 10 节）。
pub const MAX_CAPTURE_FAILURES: u32 = 10;

/// 当前时间：`instant` 用来算超时，`unix`（Unix 秒）用来判断二维码有没有过期。
#[derive(Debug, Clone, Copy)]
pub struct Now {
    pub instant: Instant,
    pub unix: i64,
}

impl Now {
    pub fn current() -> Self {
        Self {
            instant: Instant::now(),
            unix: jiff::Timestamp::now().as_second(),
        }
    }
}

/// 截一帧（或者没截成）的结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Observation {
    /// 截到一帧，`texts` 是里面识别出的所有二维码文本。
    Frame {
        source: SourceInfo,
        texts: Vec<String>,
    },
    /// 自动模式下还没找到游戏窗口，或者游戏窗口关了。
    TargetMissing,
    /// 窗口最小化了。
    TargetMinimized,
    /// 自动模式下游戏窗口不在最前面。
    TargetInBackground,
    /// 指定的窗口关了，或者指定的显示器断开了。
    TargetGone,
    /// 这一帧没截成。
    CaptureFailed(String),
}

/// 截一帧并识别。截图线程反复调用它，把结果交给会话。
pub fn observe(source: &mut dyn FrameSource, decoder: &dyn QrDecoder) -> Observation {
    match source.next_frame() {
        Ok(frame) => Observation::Frame {
            source: source.describe(),
            texts: decoder.decode(frame),
        },
        Err(CaptureError::NotFound) => Observation::TargetMissing,
        Err(CaptureError::Minimized) => Observation::TargetMinimized,
        Err(CaptureError::InBackground) => Observation::TargetInBackground,
        Err(CaptureError::Gone) => Observation::TargetGone,
        Err(CaptureError::Failed(message)) => Observation::CaptureFailed(message),
    }
}

/// 接口调用失败的原因。运行循环把接口错误分好类再喂进来（设计文档第 10 节）。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Failure {
    /// 凭证失效（retcode -100），扫描要停止。
    #[error("账号凭证已失效")]
    CredentialInvalid,
    /// 二维码过期或已被使用。
    #[error("二维码已过期")]
    QrExpired,
    /// 服务器认定的登录目标不是这个游戏。
    #[error("服务器说这是“{app_name}”的登录码")]
    AppMismatch { app_name: String },
    /// 其他错误，比如网络错误、未知错误码。内容是给人看的原因。
    #[error("{0}")]
    Other(String),
}

/// 喂给会话的事件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// 会话开始时检查凭证的结果。
    CredentialChecked(Result<(), Failure>),
    /// 截图线程报告的一帧。
    Observed(Observation),
    /// 你对确认提问的回答，`true` 表示登录。
    Answered(bool),
    /// 登录三步的结果。
    LoginFinished(Result<(), Failure>),
    /// 时间流逝。运行循环在一段时间没有新画面时喂一次，用来判断超时。
    Tick,
    /// 你要求停止。
    StopRequested,
}

/// 会话要运行循环做的事。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// 检查凭证，结果用 [`Event::CredentialChecked`] 喂回来。
    CheckCredential,
    /// 问你要不要登录，回答用 [`Event::Answered`] 喂回来。
    Ask(Prompt),
    /// 执行登录三步，结果用 [`Event::LoginFinished`] 喂回来。
    Login(LoginQr),
    /// 会话结束，停止截图。结束原因在状态快照里。
    Finish,
}

/// 确认提问的内容（设计文档第 6.2 节第 5 步）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Prompt {
    /// 游戏名，取自程序自己的游戏配置，不用二维码里的文字。
    pub game: &'static str,
    /// 画面来源，比如 `YuanShen.exe（原神）`。
    pub source: String,
    /// 怎么称呼要登录的账号，比如 `账号「大号」（原神 UID 100000001，通行证 300012345）`，
    /// 见 `Account::describe`。ID 都完整显示。
    pub account: String,
}

impl Prompt {
    /// 确认框里的问题。
    pub fn question(&self) -> String {
        format!(
            "在 {}中发现{}登录二维码。用{}登录？确认后，显示这个二维码的设备将登录你的账号。",
            self.source, self.game, self.account
        )
    }
}

/// 扫描会话的状态快照（设计文档第 6.3 节）。界面只按它显示。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ScanStatus {
    /// 没在扫描，`last` 是上次结束的原因。
    Idle { last: Option<StopReason> },
    /// 检查账号凭证。
    CheckingCredential,
    /// 自动模式下等游戏窗口出现，`game` 是游戏名。
    WaitingForWindow { game: String },
    /// 窗口最小化了，恢复后继续。
    Paused,
    /// 自动模式下游戏窗口不在最前面，切回游戏后继续。`game` 是游戏名。
    GameInBackground { game: String },
    /// 正在扫描，`notice` 是最近一条提示。
    Scanning {
        source: String,
        notice: Option<Notice>,
    },
    /// 等你确认。这期间不处理新的画面。
    AwaitingConfirm { prompt: Prompt },
    /// 正在登录。这期间不处理新的画面。
    LoggingIn,
}

impl ScanStatus {
    /// 扫描中附带的最近一条提示。
    pub fn notice(&self) -> Option<&Notice> {
        match self {
            ScanStatus::Scanning { notice, .. } => notice.as_ref(),
            _ => None,
        }
    }
}

impl fmt::Display for ScanStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ScanStatus::Idle { last: None } => write!(f, "未在扫描"),
            ScanStatus::Idle { last: Some(reason) } => write!(f, "已停止：{reason}"),
            ScanStatus::CheckingCredential => write!(f, "检查账号凭证…"),
            ScanStatus::WaitingForWindow { game } => write!(f, "等待{game}窗口…"),
            ScanStatus::Paused => write!(f, "窗口已最小化，恢复后继续"),
            ScanStatus::GameInBackground { game } => {
                write!(f, "{game}不在最前面，切回游戏后继续")
            }
            ScanStatus::Scanning { source, .. } => write!(f, "正在扫描 {source}"),
            ScanStatus::AwaitingConfirm { .. } => write!(f, "等你确认是否登录"),
            ScanStatus::LoggingIn => write!(f, "正在登录…"),
        }
    }
}

/// 扫描结束的原因。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum StopReason {
    LoggedIn { game: String },
    UserStopped,
    IdleTimeout { secs: u64 },
    CredentialInvalid,
    TargetGone,
    CaptureFailing { message: String },
    CredentialCheckFailed { message: String },
}

impl fmt::Display for StopReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StopReason::LoggedIn { game } => {
                write!(f, "登录成功，显示二维码的{game}已登录你的账号")
            }
            StopReason::UserStopped => write!(f, "你手动停止了扫描"),
            StopReason::IdleTimeout { secs } => write!(f, "连续 {secs} 秒没发现登录二维码"),
            StopReason::CredentialInvalid => write!(f, "账号凭证已失效，请重新绑定"),
            StopReason::TargetGone => write!(f, "指定的窗口已关闭，或指定的显示器已断开"),
            StopReason::CaptureFailing { message } => {
                write!(f, "截图连续失败 {MAX_CAPTURE_FAILURES} 次：{message}")
            }
            StopReason::CredentialCheckFailed { message } => {
                write!(f, "检查账号凭证时出错：{message}")
            }
        }
    }
}

/// 扫描中的提示，不会让扫描停下。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Notice {
    MultipleQrs,
    QrExpired,
    AppMismatch { app_name: String, game: String },
    Declined,
    LoginFailed { message: String },
}

impl fmt::Display for Notice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Notice::MultipleQrs => write!(f, "画面里有多个登录二维码，无法确定要登录哪一个"),
            Notice::QrExpired => write!(f, "二维码已过期或已被使用，等游戏刷新"),
            Notice::AppMismatch { app_name, game } => {
                write!(f, "服务器说这是“{app_name}”的登录码，不是{game}，已取消")
            }
            Notice::Declined => write!(f, "你取消了这次登录，同一个码不会再问"),
            Notice::LoginFailed { message } => write!(f, "登录失败：{message}"),
        }
    }
}

/// 一次扫码会话。
pub struct ScanSession {
    /// 开始时选的截图目标。自动模式下 `label` 是游戏名。
    target: SourceInfo,
    /// 确认框里怎么称呼要登录的账号。
    account: String,
    idle_timeout: Duration,
    policy: Box<dyn ConfirmPolicy + Send>,
    status: ScanStatus,
    /// 超时从这个时间点算起：会话开始、上一次发现新的合法登录码、你回答、登录结束。
    idle_since: Instant,
    capture_failures: u32,
    /// 本次会话处理过的 ticket，同一个只处理一次（设计文档第 6.2 节第 4 步）。
    seen: HashSet<String>,
    /// 本次会话在画面里见过的 ticket。只有新出现的码才让超时重新计时，
    /// 否则一个处理过的码一直留在画面上，扫描就永远不会超时。
    recognized: HashSet<String>,
    /// 正在确认或登录的二维码。
    pending: Option<LoginQr>,
    /// 登录中途你要求停止：等登录结果出来再停。
    stop_requested: bool,
    /// 最近一帧画面的来源，回到扫描状态时显示。
    source_label: String,
    notice: Option<Notice>,
}

impl ScanSession {
    /// 开始一个会话。第一个动作总是检查凭证（设计文档第 6.2 节）。
    pub fn start(
        target: SourceInfo,
        account: String,
        idle_timeout: Duration,
        policy: Box<dyn ConfirmPolicy + Send>,
        now: Now,
    ) -> (Self, Action) {
        let session = Self {
            source_label: target.label.clone(),
            target,
            account,
            idle_timeout,
            policy,
            status: ScanStatus::CheckingCredential,
            idle_since: now.instant,
            capture_failures: 0,
            seen: HashSet::new(),
            recognized: HashSet::new(),
            pending: None,
            stop_requested: false,
            notice: None,
        };
        (session, Action::CheckCredential)
    }

    pub fn status(&self) -> &ScanStatus {
        &self.status
    }

    pub fn is_finished(&self) -> bool {
        matches!(self.status, ScanStatus::Idle { .. })
    }

    /// 处理一个事件，返回下一步要做的事。会话结束后不再处理任何事件。
    pub fn handle(&mut self, event: Event, now: Now) -> Option<Action> {
        if self.is_finished() {
            return None;
        }
        if matches!(event, Event::Tick | Event::Observed(_)) && self.idle_expired(now) {
            let secs = self.idle_timeout.as_secs();
            return Some(self.finish(StopReason::IdleTimeout { secs }));
        }
        match event {
            Event::CredentialChecked(result) => self.on_credential_checked(result, now),
            Event::Observed(observation) => self.on_observed(observation, now),
            Event::Answered(yes) => self.on_answered(yes, now),
            Event::LoginFinished(result) => self.on_login_finished(result, now),
            Event::Tick => None,
            Event::StopRequested => self.on_stop_requested(),
        }
    }

    /// 是否在看画面：等窗口、暂停、游戏不在最前面、扫描。只有这时才处理画面，
    /// 也只有这时才计算超时。
    fn watching(&self) -> bool {
        matches!(
            self.status,
            ScanStatus::WaitingForWindow { .. }
                | ScanStatus::Paused
                | ScanStatus::GameInBackground { .. }
                | ScanStatus::Scanning { .. }
        )
    }

    fn idle_expired(&self, now: Now) -> bool {
        self.watching() && now.instant.duration_since(self.idle_since) >= self.idle_timeout
    }

    fn finish(&mut self, reason: StopReason) -> Action {
        self.status = ScanStatus::Idle { last: Some(reason) };
        self.pending = None;
        Action::Finish
    }

    fn on_stop_requested(&mut self) -> Option<Action> {
        // 登录请求已经发出去了，等结果出来再停，免得登录成功了却显示成“手动停止”。
        if self.status == ScanStatus::LoggingIn {
            self.stop_requested = true;
            return None;
        }
        Some(self.finish(StopReason::UserStopped))
    }

    fn show_scanning(&mut self) {
        self.status = ScanStatus::Scanning {
            source: self.source_label.clone(),
            notice: self.notice.clone(),
        };
    }

    fn show_waiting(&mut self) {
        self.status = ScanStatus::WaitingForWindow {
            game: self.target.label.clone(),
        };
    }

    fn on_credential_checked(&mut self, result: Result<(), Failure>, now: Now) -> Option<Action> {
        if self.status != ScanStatus::CheckingCredential {
            return None;
        }
        match result {
            Ok(()) => {
                self.idle_since = now.instant;
                if self.target.kind == SourceKind::GameWindow {
                    self.show_waiting();
                } else {
                    self.show_scanning();
                }
                None
            }
            Err(Failure::CredentialInvalid) => Some(self.finish(StopReason::CredentialInvalid)),
            Err(other) => Some(self.finish(StopReason::CredentialCheckFailed {
                message: printable(&other.to_string()),
            })),
        }
    }

    fn on_observed(&mut self, observation: Observation, now: Now) -> Option<Action> {
        // 检查凭证、等你确认、登录期间不处理画面（设计文档第 6.2 节第 2 步）。
        if !self.watching() {
            return None;
        }
        match observation {
            Observation::TargetMissing => {
                self.show_waiting();
                None
            }
            Observation::TargetMinimized => {
                self.status = ScanStatus::Paused;
                None
            }
            Observation::TargetInBackground => {
                self.status = ScanStatus::GameInBackground {
                    game: self.target.label.clone(),
                };
                None
            }
            // 自动模式下游戏窗口关了，游戏可能会重开，回去等着；指定的目标没了就停止。
            Observation::TargetGone if self.target.kind == SourceKind::GameWindow => {
                self.show_waiting();
                None
            }
            Observation::TargetGone => Some(self.finish(StopReason::TargetGone)),
            Observation::CaptureFailed(message) => {
                self.capture_failures += 1;
                if self.capture_failures >= MAX_CAPTURE_FAILURES {
                    Some(self.finish(StopReason::CaptureFailing {
                        message: printable(&message),
                    }))
                } else {
                    None
                }
            }
            Observation::Frame { source, texts } => {
                self.capture_failures = 0;
                self.on_frame(source, &texts, now)
            }
        }
    }

    fn on_frame(&mut self, source: SourceInfo, texts: &[String], now: Now) -> Option<Action> {
        self.source_label = source.label.clone();
        // 同一个码在一帧里可能被识别出两次，按 ticket 去重。
        let mut codes: Vec<LoginQr> = Vec::new();
        for text in texts {
            if let Ok(qr) = parse_login_qr(text, now.unix)
                && !codes.iter().any(|code| code.ticket == qr.ticket)
            {
                codes.push(qr);
            }
        }
        if codes.is_empty() {
            self.show_scanning();
            return None;
        }
        // 画面里出现了新的合法登录码，超时重新计时。一直留在画面上的旧码不算。
        let mut found_new = false;
        for code in &codes {
            found_new |= self.recognized.insert(code.ticket.clone());
        }
        if found_new {
            self.idle_since = now.instant;
        }
        if codes.len() > 1 {
            // 不弹窗：确认框里的文字分辨不出是哪一个（设计文档第 6.2 节第 2 步）。
            self.notice = Some(Notice::MultipleQrs);
            self.show_scanning();
            return None;
        }
        let qr = codes.remove(0);
        if !self.seen.insert(qr.ticket.clone()) {
            self.show_scanning();
            return None;
        }
        // 硬性规则既看画面自己报的来源，也看你开始时选的目标，取严格的那个。
        let decision = enforce_hard_rules(self.policy.decide(&source, &qr), &source);
        match enforce_hard_rules(decision, &self.target) {
            Decision::Ignore => {
                self.show_scanning();
                None
            }
            Decision::AskUser => {
                let prompt = Prompt {
                    game: qr.game.display_name,
                    source: source.label,
                    account: self.account.clone(),
                };
                self.status = ScanStatus::AwaitingConfirm {
                    prompt: prompt.clone(),
                };
                self.pending = Some(qr);
                Some(Action::Ask(prompt))
            }
            Decision::AutoConfirm => {
                self.status = ScanStatus::LoggingIn;
                self.pending = Some(qr.clone());
                Some(Action::Login(qr))
            }
        }
    }

    fn on_answered(&mut self, yes: bool, now: Now) -> Option<Action> {
        if !matches!(self.status, ScanStatus::AwaitingConfirm { .. }) {
            return None;
        }
        self.idle_since = now.instant;
        match (yes, self.pending.clone()) {
            (true, Some(qr)) => {
                self.status = ScanStatus::LoggingIn;
                Some(Action::Login(qr))
            }
            _ => {
                self.pending = None;
                self.notice = Some(Notice::Declined);
                self.show_scanning();
                None
            }
        }
    }

    fn on_login_finished(&mut self, result: Result<(), Failure>, now: Now) -> Option<Action> {
        if self.status != ScanStatus::LoggingIn {
            return None;
        }
        let game = self.pending.take().map_or("", |qr| qr.game.display_name);
        self.idle_since = now.instant;
        // 只有凭证失效才停止；其他失败只影响这一个二维码（设计文档第 6.2 节第 7 步）。
        let notice = match result {
            Ok(()) => {
                return Some(self.finish(StopReason::LoggedIn {
                    game: game.to_owned(),
                }));
            }
            Err(Failure::CredentialInvalid) => {
                return Some(self.finish(StopReason::CredentialInvalid));
            }
            Err(Failure::QrExpired) => Notice::QrExpired,
            Err(Failure::AppMismatch { app_name }) => Notice::AppMismatch {
                app_name: printable(&app_name),
                game: game.to_owned(),
            },
            Err(Failure::Other(message)) => Notice::LoginFailed {
                message: printable(&message),
            },
        };
        if self.stop_requested {
            return Some(self.finish(StopReason::UserStopped));
        }
        self.notice = Some(notice);
        self.show_scanning();
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capture::Frame;
    use crate::confirm::AlwaysAsk;

    const VALID: &str = "https://user.mihoyo.com/qr_code_in_game.html?app_id=4&biz_key=hk4e_cn&expire=4102444800&ticket=66cf3afb2a1c7a308832bd56";
    const ACCOUNT: &str = "账号「大号」（原神 UID 100000001，通行证 300012345）";
    const OTHER: &str = "https://user.mihoyo.com/qr_code_in_game.html?app_id=4&biz_key=hk4e_cn&expire=4102444800&ticket=77cf3afb2a1c7a308832bd57";
    const TIMEOUT: Duration = Duration::from_secs(300);

    /// 测试用的时钟：`at(秒)` 是会话开始后的第几秒。
    struct Clock(Instant);

    impl Clock {
        fn new() -> Self {
            Self(Instant::now())
        }

        fn at(&self, secs: u64) -> Now {
            Now {
                instant: self.0 + Duration::from_secs(secs),
                unix: 1_760_000_000 + secs as i64,
            }
        }
    }

    fn game_window() -> SourceInfo {
        SourceInfo::new(SourceKind::GameWindow, "原神")
    }

    fn found_game() -> SourceInfo {
        SourceInfo::new(SourceKind::GameWindow, "YuanShen.exe（原神）")
    }

    fn picked_window() -> SourceInfo {
        SourceInfo::new(SourceKind::PickedWindow, "直播间（chrome.exe）")
    }

    fn monitor() -> SourceInfo {
        SourceInfo::new(SourceKind::Monitor, "显示器 DELL（3840×2160）")
    }

    fn frame(source: SourceInfo, texts: &[&str]) -> Event {
        Event::Observed(Observation::Frame {
            source,
            texts: texts.iter().map(|t| (*t).to_owned()).collect(),
        })
    }

    /// 凭证检查已经通过的会话。
    fn start(
        target: SourceInfo,
        policy: Box<dyn ConfirmPolicy + Send>,
        clock: &Clock,
    ) -> ScanSession {
        let (mut session, first) =
            ScanSession::start(target, ACCOUNT.to_owned(), TIMEOUT, policy, clock.at(0));
        assert_eq!(first, Action::CheckCredential);
        assert_eq!(
            session.handle(Event::CredentialChecked(Ok(())), clock.at(0)),
            None
        );
        session
    }

    fn stop_reason(session: &ScanSession) -> Option<StopReason> {
        match session.status() {
            ScanStatus::Idle { last } => last.clone(),
            _ => None,
        }
    }

    fn asked(action: Option<Action>) -> Prompt {
        match action {
            Some(Action::Ask(prompt)) => prompt,
            other => panic!("应该询问，实际是 {other:?}"),
        }
    }

    fn login_ticket(action: Option<Action>) -> String {
        match action {
            Some(Action::Login(qr)) => qr.ticket,
            other => panic!("应该登录，实际是 {other:?}"),
        }
    }

    /// 问过、你也同意了，正在登录的会话。
    fn logging_in(clock: &Clock) -> ScanSession {
        let mut session = start(game_window(), Box::new(AlwaysAsk), clock);
        asked(session.handle(frame(found_game(), &[VALID]), clock.at(1)));
        login_ticket(session.handle(Event::Answered(true), clock.at(2)));
        session
    }

    /// 什么码都自动确认的策略，用来检查硬性规则。
    struct AutoConfirmAll;

    impl ConfirmPolicy for AutoConfirmAll {
        fn decide(&mut self, _source: &SourceInfo, _qr: &LoginQr) -> Decision {
            Decision::AutoConfirm
        }
    }

    // ---- 开始和停止 ----

    #[test]
    fn starts_by_checking_the_credential() {
        let clock = Clock::new();
        let (session, first) = ScanSession::start(
            game_window(),
            ACCOUNT.to_owned(),
            TIMEOUT,
            Box::new(AlwaysAsk),
            clock.at(0),
        );
        assert_eq!(first, Action::CheckCredential);
        assert_eq!(session.status(), &ScanStatus::CheckingCredential);
    }

    #[test]
    fn auto_mode_waits_for_the_game_window_after_the_check() {
        let clock = Clock::new();
        let session = start(game_window(), Box::new(AlwaysAsk), &clock);
        assert_eq!(
            session.status(),
            &ScanStatus::WaitingForWindow {
                game: "原神".to_owned()
            }
        );
    }

    #[test]
    fn picked_targets_start_scanning_after_the_check() {
        let clock = Clock::new();
        let session = start(monitor(), Box::new(AlwaysAsk), &clock);
        assert_eq!(
            session.status(),
            &ScanStatus::Scanning {
                source: "显示器 DELL（3840×2160）".to_owned(),
                notice: None,
            }
        );
    }

    #[test]
    fn invalid_credential_stops_before_scanning() {
        let clock = Clock::new();
        let (mut session, _) = ScanSession::start(
            game_window(),
            ACCOUNT.to_owned(),
            TIMEOUT,
            Box::new(AlwaysAsk),
            clock.at(0),
        );
        let action = session.handle(
            Event::CredentialChecked(Err(Failure::CredentialInvalid)),
            clock.at(1),
        );
        assert_eq!(action, Some(Action::Finish));
        assert_eq!(stop_reason(&session), Some(StopReason::CredentialInvalid));
    }

    #[test]
    fn other_check_errors_stop_with_the_reason() {
        let clock = Clock::new();
        let (mut session, _) = ScanSession::start(
            game_window(),
            ACCOUNT.to_owned(),
            TIMEOUT,
            Box::new(AlwaysAsk),
            clock.at(0),
        );
        let failure = Failure::Other("网络请求失败".to_owned());
        assert_eq!(
            session.handle(Event::CredentialChecked(Err(failure)), clock.at(1)),
            Some(Action::Finish)
        );
        assert_eq!(
            stop_reason(&session),
            Some(StopReason::CredentialCheckFailed {
                message: "网络请求失败".to_owned()
            })
        );
    }

    #[test]
    fn stopping_ends_the_session_and_later_events_are_ignored() {
        let clock = Clock::new();
        let mut session = start(game_window(), Box::new(AlwaysAsk), &clock);
        assert_eq!(
            session.handle(Event::StopRequested, clock.at(1)),
            Some(Action::Finish)
        );
        assert!(session.is_finished());
        assert_eq!(
            session.handle(frame(found_game(), &[VALID]), clock.at(2)),
            None
        );
        assert_eq!(stop_reason(&session), Some(StopReason::UserStopped));
    }

    #[test]
    fn status_is_a_tagged_snapshot() {
        let waiting = ScanStatus::WaitingForWindow {
            game: "原神".to_owned(),
        };
        assert_eq!(
            serde_json::to_value(&waiting).unwrap(),
            serde_json::json!({"kind": "waiting_for_window", "game": "原神"})
        );
        let stopped = ScanStatus::Idle {
            last: Some(StopReason::IdleTimeout { secs: 300 }),
        };
        assert_eq!(
            serde_json::to_value(&stopped).unwrap(),
            serde_json::json!({"kind": "idle", "last": {"kind": "idle_timeout", "secs": 300}})
        );
    }

    #[test]
    fn status_text_matches_the_design() {
        let waiting = ScanStatus::WaitingForWindow {
            game: "原神".to_owned(),
        };
        assert_eq!(waiting.to_string(), "等待原神窗口…");
        assert_eq!(ScanStatus::Paused.to_string(), "窗口已最小化，恢复后继续");
        let stopped = ScanStatus::Idle {
            last: Some(StopReason::IdleTimeout { secs: 300 }),
        };
        assert_eq!(stopped.to_string(), "已停止：连续 300 秒没发现登录二维码");
    }

    // ---- 处理画面 ----

    #[test]
    fn asks_before_logging_in() {
        let clock = Clock::new();
        let mut session = start(game_window(), Box::new(AlwaysAsk), &clock);
        let prompt = asked(session.handle(frame(found_game(), &[VALID]), clock.at(1)));
        assert_eq!(
            prompt.question(),
            "在 YuanShen.exe（原神）中发现原神登录二维码。用账号「大号」（原神 UID 100000001，通行证 300012345）登录？确认后，显示这个二维码的设备将登录你的账号。"
        );
        assert!(matches!(
            session.status(),
            ScanStatus::AwaitingConfirm { .. }
        ));
        let ticket = login_ticket(session.handle(Event::Answered(true), clock.at(2)));
        assert_eq!(ticket, "66cf3afb2a1c7a308832bd56");
        assert_eq!(session.status(), &ScanStatus::LoggingIn);
    }

    #[test]
    fn frames_without_a_valid_login_qr_are_ignored() {
        let clock = Clock::new();
        let mut session = start(game_window(), Box::new(AlwaysAsk), &clock);
        let expired = VALID.replace("expire=4102444800", "expire=1000000000");
        assert_eq!(
            session.handle(
                frame(found_game(), &["你好", expired.as_str()]),
                clock.at(1)
            ),
            None
        );
        assert_eq!(
            session.status(),
            &ScanStatus::Scanning {
                source: "YuanShen.exe（原神）".to_owned(),
                notice: None,
            }
        );
    }

    #[test]
    fn each_ticket_is_handled_once_and_declined_ones_are_not_asked_again() {
        let clock = Clock::new();
        let mut session = start(game_window(), Box::new(AlwaysAsk), &clock);
        asked(session.handle(frame(found_game(), &[VALID]), clock.at(1)));
        assert_eq!(session.handle(Event::Answered(false), clock.at(2)), None);
        assert_eq!(session.status().notice(), Some(&Notice::Declined));
        assert_eq!(
            session.handle(frame(found_game(), &[VALID]), clock.at(3)),
            None
        );
        // 新的码照样问。
        asked(session.handle(frame(found_game(), &[OTHER]), clock.at(4)));
    }

    #[test]
    fn several_different_login_qrs_in_one_frame_are_not_asked() {
        let clock = Clock::new();
        let mut session = start(game_window(), Box::new(AlwaysAsk), &clock);
        assert_eq!(
            session.handle(frame(found_game(), &[VALID, OTHER]), clock.at(1)),
            None
        );
        assert_eq!(session.status().notice(), Some(&Notice::MultipleQrs));
        // 画面里只剩一个时再问。
        asked(session.handle(frame(found_game(), &[OTHER]), clock.at(2)));
    }

    #[test]
    fn the_same_qr_twice_in_one_frame_counts_once() {
        let clock = Clock::new();
        let mut session = start(game_window(), Box::new(AlwaysAsk), &clock);
        asked(session.handle(frame(found_game(), &[VALID, VALID]), clock.at(1)));
    }

    #[test]
    fn frames_are_ignored_while_waiting_for_an_answer_or_logging_in() {
        let clock = Clock::new();
        let mut session = start(game_window(), Box::new(AlwaysAsk), &clock);
        asked(session.handle(frame(found_game(), &[VALID]), clock.at(1)));
        assert_eq!(
            session.handle(frame(found_game(), &[OTHER]), clock.at(2)),
            None
        );
        assert!(matches!(
            session.status(),
            ScanStatus::AwaitingConfirm { .. }
        ));
        login_ticket(session.handle(Event::Answered(true), clock.at(3)));
        assert_eq!(
            session.handle(frame(found_game(), &[OTHER]), clock.at(4)),
            None
        );
        assert_eq!(session.status(), &ScanStatus::LoggingIn);
    }

    #[test]
    fn follows_the_target_window() {
        let clock = Clock::new();
        let mut session = start(game_window(), Box::new(AlwaysAsk), &clock);
        session.handle(Event::Observed(Observation::TargetMinimized), clock.at(1));
        assert_eq!(session.status(), &ScanStatus::Paused);
        session.handle(frame(found_game(), &[]), clock.at(2));
        assert!(matches!(session.status(), ScanStatus::Scanning { .. }));
        session.handle(Event::Observed(Observation::TargetMissing), clock.at(3));
        assert_eq!(
            session.status(),
            &ScanStatus::WaitingForWindow {
                game: "原神".to_owned()
            }
        );
    }

    #[test]
    fn auto_mode_pauses_while_the_game_is_in_the_background() {
        // 截的是游戏窗口所在的屏幕区域，游戏不在最前面时可能截到盖在上面的窗口。
        let clock = Clock::new();
        let mut session = start(game_window(), Box::new(AlwaysAsk), &clock);
        session.handle(
            Event::Observed(Observation::TargetInBackground),
            clock.at(1),
        );
        let status = session.status().clone();
        assert_eq!(
            status,
            ScanStatus::GameInBackground {
                game: "原神".to_owned()
            }
        );
        assert_eq!(status.to_string(), "原神不在最前面，切回游戏后继续");
        session.handle(frame(found_game(), &[]), clock.at(2));
        assert!(matches!(session.status(), ScanStatus::Scanning { .. }));
        // 游戏一直不在最前面，照样超时。
        session.handle(
            Event::Observed(Observation::TargetInBackground),
            clock.at(3),
        );
        assert_eq!(
            session.handle(Event::Tick, clock.at(300)),
            Some(Action::Finish)
        );
    }

    #[test]
    fn auto_mode_waits_again_when_the_game_window_closes() {
        let clock = Clock::new();
        let mut session = start(game_window(), Box::new(AlwaysAsk), &clock);
        session.handle(frame(found_game(), &[]), clock.at(1));
        assert_eq!(
            session.handle(Event::Observed(Observation::TargetGone), clock.at(2)),
            None
        );
        assert_eq!(
            session.status(),
            &ScanStatus::WaitingForWindow {
                game: "原神".to_owned()
            }
        );
    }

    #[test]
    fn closing_a_picked_window_stops_the_session() {
        let clock = Clock::new();
        let mut session = start(picked_window(), Box::new(AlwaysAsk), &clock);
        assert_eq!(
            session.handle(Event::Observed(Observation::TargetGone), clock.at(1)),
            Some(Action::Finish)
        );
        assert_eq!(stop_reason(&session), Some(StopReason::TargetGone));
    }

    #[test]
    fn stops_after_ten_capture_failures_in_a_row() {
        let clock = Clock::new();
        let mut session = start(monitor(), Box::new(AlwaysAsk), &clock);
        let fail = || Event::Observed(Observation::CaptureFailed("拒绝访问".to_owned()));
        for _ in 0..9 {
            assert_eq!(session.handle(fail(), clock.at(1)), None);
        }
        // 成功截到一帧就重新计数。
        session.handle(frame(monitor(), &[]), clock.at(2));
        for _ in 0..9 {
            assert_eq!(session.handle(fail(), clock.at(3)), None);
        }
        assert_eq!(session.handle(fail(), clock.at(3)), Some(Action::Finish));
        assert_eq!(
            stop_reason(&session),
            Some(StopReason::CaptureFailing {
                message: "拒绝访问".to_owned()
            })
        );
    }

    #[test]
    fn auto_confirm_logs_in_directly_from_the_game_window() {
        let clock = Clock::new();
        let mut session = start(game_window(), Box::new(AutoConfirmAll), &clock);
        login_ticket(session.handle(frame(found_game(), &[VALID]), clock.at(1)));
        assert_eq!(session.status(), &ScanStatus::LoggingIn);
    }

    #[test]
    fn picked_windows_and_monitors_are_never_auto_confirmed() {
        for source in [picked_window(), monitor()] {
            let clock = Clock::new();
            let mut session = start(source.clone(), Box::new(AutoConfirmAll), &clock);
            asked(session.handle(frame(source, &[VALID]), clock.at(1)));
        }
    }

    #[test]
    fn hard_rules_also_look_at_the_chosen_target() {
        // 即使画面自称来自游戏窗口，你选的是指定窗口时也不能自动确认。
        let clock = Clock::new();
        let mut session = start(picked_window(), Box::new(AutoConfirmAll), &clock);
        asked(session.handle(frame(found_game(), &[VALID]), clock.at(1)));
    }

    struct FakeSource(Vec<Result<Frame, CaptureError>>);

    impl FrameSource for FakeSource {
        fn next_frame(&mut self) -> Result<Frame, CaptureError> {
            self.0.remove(0)
        }

        fn describe(&self) -> SourceInfo {
            found_game()
        }
    }

    /// 把画面尺寸当作识别结果，看得出 observe 把画面交给了识别器。
    struct FakeDecoder;

    impl QrDecoder for FakeDecoder {
        fn decode(&self, frame: Frame) -> Vec<String> {
            vec![format!("{}x{}", frame.width, frame.height)]
        }
    }

    #[test]
    fn observe_turns_capture_results_into_observations() {
        let mut source = FakeSource(vec![
            Ok(Frame {
                width: 2,
                height: 1,
                luma: vec![0, 255],
            }),
            Err(CaptureError::NotFound),
            Err(CaptureError::Minimized),
            Err(CaptureError::Gone),
            Err(CaptureError::Failed("拒绝访问".to_owned())),
            Err(CaptureError::InBackground),
        ]);
        assert_eq!(
            observe(&mut source, &FakeDecoder),
            Observation::Frame {
                source: found_game(),
                texts: vec!["2x1".to_owned()],
            }
        );
        assert_eq!(
            observe(&mut source, &FakeDecoder),
            Observation::TargetMissing
        );
        assert_eq!(
            observe(&mut source, &FakeDecoder),
            Observation::TargetMinimized
        );
        assert_eq!(observe(&mut source, &FakeDecoder), Observation::TargetGone);
        assert_eq!(
            observe(&mut source, &FakeDecoder),
            Observation::CaptureFailed("拒绝访问".to_owned())
        );
        assert_eq!(
            observe(&mut source, &FakeDecoder),
            Observation::TargetInBackground
        );
    }

    // ---- 登录结果 ----

    #[test]
    fn successful_login_ends_the_session() {
        let clock = Clock::new();
        let mut session = logging_in(&clock);
        assert_eq!(
            session.handle(Event::LoginFinished(Ok(())), clock.at(3)),
            Some(Action::Finish)
        );
        assert_eq!(
            stop_reason(&session),
            Some(StopReason::LoggedIn {
                game: "原神".to_owned()
            })
        );
    }

    #[test]
    fn invalid_credential_during_login_stops() {
        let clock = Clock::new();
        let mut session = logging_in(&clock);
        assert_eq!(
            session.handle(
                Event::LoginFinished(Err(Failure::CredentialInvalid)),
                clock.at(3)
            ),
            Some(Action::Finish)
        );
        assert_eq!(stop_reason(&session), Some(StopReason::CredentialInvalid));
    }

    #[test]
    fn other_login_failures_keep_scanning_without_retrying() {
        let cases = [
            (Failure::QrExpired, Notice::QrExpired),
            (
                Failure::AppMismatch {
                    app_name: "崩坏：\r星穹铁道".to_owned(),
                },
                Notice::AppMismatch {
                    app_name: "崩坏： 星穹铁道".to_owned(),
                    game: "原神".to_owned(),
                },
            ),
            (
                Failure::Other("网络请求失败".to_owned()),
                Notice::LoginFailed {
                    message: "网络请求失败".to_owned(),
                },
            ),
        ];
        for (failure, notice) in cases {
            let clock = Clock::new();
            let mut session = logging_in(&clock);
            assert_eq!(
                session.handle(Event::LoginFinished(Err(failure)), clock.at(3)),
                None
            );
            assert_eq!(
                session.status(),
                &ScanStatus::Scanning {
                    source: "YuanShen.exe（原神）".to_owned(),
                    notice: Some(notice),
                }
            );
            // 同一个码不会再试。
            assert_eq!(
                session.handle(frame(found_game(), &[VALID]), clock.at(4)),
                None
            );
        }
    }

    #[test]
    fn stopping_while_logging_in_waits_for_the_result() {
        // 登录请求已经发出去了，停止不能让人以为没登录。
        let clock = Clock::new();
        let mut session = logging_in(&clock);
        assert_eq!(session.handle(Event::StopRequested, clock.at(3)), None);
        assert_eq!(session.status(), &ScanStatus::LoggingIn);
        assert_eq!(
            session.handle(Event::LoginFinished(Ok(())), clock.at(4)),
            Some(Action::Finish)
        );
        assert_eq!(
            stop_reason(&session),
            Some(StopReason::LoggedIn {
                game: "原神".to_owned()
            })
        );
    }

    #[test]
    fn stopping_while_logging_in_stops_after_a_failed_login() {
        let clock = Clock::new();
        let mut session = logging_in(&clock);
        session.handle(Event::StopRequested, clock.at(3));
        assert_eq!(
            session.handle(Event::LoginFinished(Err(Failure::QrExpired)), clock.at(4)),
            Some(Action::Finish)
        );
        assert_eq!(stop_reason(&session), Some(StopReason::UserStopped));
    }

    // ---- 超时 ----

    #[test]
    fn a_handled_qr_left_on_screen_does_not_keep_the_session_alive() {
        let clock = Clock::new();
        let mut session = start(game_window(), Box::new(AlwaysAsk), &clock);
        asked(session.handle(frame(found_game(), &[VALID]), clock.at(10)));
        session.handle(Event::Answered(false), clock.at(20));
        // 拒绝过的码一直留在画面上，不再重新计时。
        assert_eq!(
            session.handle(frame(found_game(), &[VALID]), clock.at(200)),
            None
        );
        assert_eq!(
            session.handle(frame(found_game(), &[VALID]), clock.at(320)),
            Some(Action::Finish)
        );
    }

    #[test]
    fn several_codes_left_on_screen_do_not_keep_the_session_alive() {
        let clock = Clock::new();
        let mut session = start(game_window(), Box::new(AlwaysAsk), &clock);
        assert_eq!(
            session.handle(frame(found_game(), &[VALID, OTHER]), clock.at(10)),
            None
        );
        assert_eq!(
            session.handle(frame(found_game(), &[VALID, OTHER]), clock.at(200)),
            None
        );
        assert_eq!(
            session.handle(frame(found_game(), &[VALID, OTHER]), clock.at(310)),
            Some(Action::Finish)
        );
    }

    #[test]
    fn stops_after_the_idle_timeout() {
        let clock = Clock::new();
        let mut session = start(game_window(), Box::new(AlwaysAsk), &clock);
        assert_eq!(session.handle(Event::Tick, clock.at(299)), None);
        assert_eq!(
            session.handle(Event::Tick, clock.at(300)),
            Some(Action::Finish)
        );
        assert_eq!(
            stop_reason(&session),
            Some(StopReason::IdleTimeout { secs: 300 })
        );
    }

    #[test]
    fn paused_time_counts_towards_the_timeout() {
        let clock = Clock::new();
        let mut session = start(game_window(), Box::new(AlwaysAsk), &clock);
        session.handle(Event::Observed(Observation::TargetMinimized), clock.at(100));
        assert_eq!(
            session.handle(Event::Observed(Observation::TargetMinimized), clock.at(300)),
            Some(Action::Finish)
        );
    }

    #[test]
    fn finding_a_login_qr_restarts_the_timer() {
        let clock = Clock::new();
        let mut session = start(game_window(), Box::new(AlwaysAsk), &clock);
        asked(session.handle(frame(found_game(), &[VALID]), clock.at(200)));
        assert_eq!(session.handle(Event::Answered(false), clock.at(210)), None);
        assert_eq!(session.handle(Event::Tick, clock.at(509)), None);
        assert_eq!(
            session.handle(Event::Tick, clock.at(510)),
            Some(Action::Finish)
        );
    }

    #[test]
    fn no_timeout_while_waiting_for_an_answer() {
        let clock = Clock::new();
        let mut session = start(game_window(), Box::new(AlwaysAsk), &clock);
        asked(session.handle(frame(found_game(), &[VALID]), clock.at(10)));
        assert_eq!(session.handle(Event::Tick, clock.at(1000)), None);
        assert!(matches!(
            session.status(),
            ScanStatus::AwaitingConfirm { .. }
        ));
    }
}
