//! 扫码会话的运行循环（设计文档第 6.2、6.3 节）：截图线程、主循环、执行会话的动作。
//! 命令行（`cli watch`）和图形界面（`app`）共用，区别只在怎么提问、怎么显示状态（[`ScanUi`]）。
#![forbid(unsafe_code)]

use std::collections::VecDeque;
use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use mhy_carry_api::{ApiClient, ApiError, Credentials};
use mhy_carry_core::account::{GameRole, SecretString};
use mhy_carry_core::capture::FrameSource;
use mhy_carry_core::confirm::AlwaysAsk;
use mhy_carry_core::decode::QrDecoder;
use mhy_carry_core::qr::LoginQr;
use mhy_carry_core::scan::{
    Action, Event, Failure, Now, Observation, Prompt, ScanSession, ScanStatus, observe,
};
use tokio::sync::mpsc;

/// 每秒最多 4 帧（设计文档第 6.2 节）。
const FRAME_INTERVAL: Duration = Duration::from_millis(250);
/// 自动模式下找不到游戏窗口时，每 2 秒重找一次。
const SEARCH_INTERVAL: Duration = Duration::from_secs(2);
/// 这么久没有新画面时喂一次 Tick，让会话检查超时。
const TICK: Duration = Duration::from_secs(1);
/// 查原神角色最多等这么久（含重试）。它只是锦上添花，不能拖慢开始扫描（设计文档第 6.4 节）。
const ROLE_LOOKUP_LIMIT: Duration = Duration::from_secs(5);

/// 怎么提问、怎么显示状态。命令行打印到终端；图形界面弹系统确认框、推送状态快照。
pub trait ScanUi: Sync {
    /// 每处理完一个事件调用一次，状态不一定有变化。
    fn show(&self, status: &ScanStatus);
    /// 问你要不要登录，返回 true 表示登录。
    fn ask(&self, prompt: Prompt) -> impl Future<Output = bool> + Send;
    /// 检查凭证时顺便查到了原神角色（设计文档第 6.4 节）。默认什么也不做。
    fn role_found(&self, _role: &GameRole) {}
}

/// 检查凭证和扫码登录（设计文档第 5 节的 `LoginApi`）。正式实现是 [`ApiClient`]。
pub trait LoginApi: Sync {
    /// 检查凭证，顺便查原神角色。凭证有问题时返回 Err；查角色失败不算错误，返回 Ok(None)。
    fn check_credential(
        &self,
        uid: &str,
        credentials: &Credentials<'_>,
    ) -> impl Future<Output = Result<Option<GameRole>, Failure>> + Send;
    fn login(
        &self,
        qr: &LoginQr,
        credentials: &Credentials<'_>,
    ) -> impl Future<Output = Result<(), Failure>> + Send;
}

impl LoginApi for ApiClient {
    async fn check_credential(
        &self,
        uid: &str,
        credentials: &Credentials<'_>,
    ) -> Result<Option<GameRole>, Failure> {
        // 检查凭证没有副作用，网络出错时重试一次（设计文档第 10 节）。
        let token = match self.check_stoken(credentials).await {
            Err(ApiError::Network(_)) => self.check_stoken(credentials).await,
            result => result,
        }
        .map_err(Failure::from)?;
        let Some(token) = token else {
            return Ok(None);
        };
        // 查角色也只是查询，网络出错重试一次；再失败或者超时也不影响扫码（第 6.4 节）。
        let lookup = async {
            match self.genshin_role(uid, &token).await {
                Err(ApiError::Network(_)) => self.genshin_role(uid, &token).await,
                result => result,
            }
        };
        let role = tokio::time::timeout(ROLE_LOOKUP_LIMIT, lookup).await;
        Ok(role.ok().and_then(Result::ok).flatten())
    }

    async fn login(&self, qr: &LoginQr, credentials: &Credentials<'_>) -> Result<(), Failure> {
        self.login_with_qr(qr, credentials)
            .await
            .map_err(Failure::from)
    }
}

/// 一次扫描要用到的东西。
pub struct ScanJob {
    pub source: Box<dyn FrameSource + Send>,
    pub decoder: Box<dyn QrDecoder + Send>,
    /// 确认框里怎么称呼这个账号，见 `Account::describe`。
    pub account: String,
    pub uid: String,
    pub mid: String,
    pub stoken: SecretString,
    pub idle_timeout: Duration,
}

/// 跑一次扫码会话，直到它结束，返回结束时的状态快照。
/// `stop` 收到消息时请求停止；登录请求已经发出时，会等登录结果出来再停。
pub async fn run_scan(
    api: &impl LoginApi,
    job: ScanJob,
    ui: &impl ScanUi,
    mut stop: mpsc::Receiver<()>,
) -> ScanStatus {
    let ScanJob {
        source,
        decoder,
        account,
        uid,
        mid,
        stoken,
        idle_timeout,
    } = job;
    let credentials = Credentials {
        stoken: &stoken,
        mid: &mid,
    };
    let (mut session, first) = ScanSession::start(
        source.describe(),
        account,
        idle_timeout,
        Box::new(AlwaysAsk),
        Now::current(),
    );
    ui.show(session.status());

    // 检查凭证、等你回答、登录期间不处理画面（设计文档第 6.2 节第 2 步），截图线程也跟着停下。
    let busy = Arc::new(AtomicBool::new(false));
    let mut frames = spawn_capture(source, decoder, Arc::clone(&busy));
    let mut actions = VecDeque::from([first]);
    loop {
        if !actions.is_empty() {
            busy.store(true, Ordering::Relaxed);
            while let Some(action) = actions.pop_front() {
                let event = match action {
                    Action::CheckCredential => {
                        let result = api.check_credential(&uid, &credentials).await;
                        if let Ok(Some(role)) = &result {
                            ui.role_found(role);
                        }
                        Event::CredentialChecked(result.map(|_| ()))
                    }
                    Action::Ask(prompt) => Event::Answered(ui.ask(prompt).await),
                    Action::Login(qr) => Event::LoginFinished(api.login(&qr, &credentials).await),
                    Action::Finish => return session.status().clone(),
                };
                actions.extend(session.handle(event, Now::current()));
                ui.show(session.status());
            }
            busy.store(false, Ordering::Relaxed);
            // 动作之前截到、还留在通道里的画面已经过时，丢掉，只处理之后截到的。
            while frames.try_recv().is_ok() {}
        }
        // “停止”优先：和新画面同时就绪时先处理停止，免得点了停止还弹出登录确认框。
        let event = tokio::select! {
            biased;
            Some(()) = stop.recv() => Event::StopRequested,
            observed = tokio::time::timeout(TICK, frames.recv()) => match observed {
                Ok(Some(observation)) => Event::Observed(observation),
                Ok(None) => Event::Observed(Observation::CaptureFailed("截图线程意外退出".to_owned())),
                Err(_) => Event::Tick,
            },
        };
        actions.extend(session.handle(event, Now::current()));
        ui.show(session.status());
    }
}

/// 在单独的线程里截图和识别（设计文档第 6.2 节第 2 步）。结果放进容量为 1 的通道，
/// 会话来不及处理时直接丢掉这一帧。`busy` 为真时不截图。会话结束后通道关闭，线程跟着退出。
fn spawn_capture(
    mut source: Box<dyn FrameSource + Send>,
    decoder: Box<dyn QrDecoder + Send>,
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
            let observation = observe(source.as_mut(), decoder.as_ref());
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use std::sync::atomic::AtomicU32;

    use mhy_carry_core::capture::{CaptureError, Frame, SourceInfo, SourceKind};
    use mhy_carry_core::scan::StopReason;

    const VALID: &str = "https://user.mihoyo.com/qr_code_in_game.html?app_id=4&biz_key=hk4e_cn&expire=4102444800&ticket=66cf3afb2a1c7a308832bd56";

    /// 一直能截到画面的游戏窗口。
    struct FakeSource;

    impl FrameSource for FakeSource {
        fn next_frame(&mut self) -> Result<Frame, CaptureError> {
            Ok(Frame {
                width: 1,
                height: 1,
                luma: vec![0],
            })
        }

        fn describe(&self) -> SourceInfo {
            SourceInfo::new(SourceKind::GameWindow, "YuanShen.exe（原神）")
        }
    }

    /// 每一帧都“识别出”同一个登录码。
    struct FakeDecoder;

    impl QrDecoder for FakeDecoder {
        fn decode(&self, _frame: Frame) -> Vec<String> {
            vec![VALID.to_owned()]
        }
    }

    struct FakeApi {
        check: Result<Option<GameRole>, Failure>,
        logins: AtomicU32,
    }

    impl FakeApi {
        fn new(check: Result<Option<GameRole>, Failure>) -> Self {
            Self {
                check,
                logins: AtomicU32::new(0),
            }
        }
    }

    impl LoginApi for FakeApi {
        async fn check_credential(
            &self,
            _uid: &str,
            _credentials: &Credentials<'_>,
        ) -> Result<Option<GameRole>, Failure> {
            self.check.clone()
        }

        async fn login(
            &self,
            _qr: &LoginQr,
            _credentials: &Credentials<'_>,
        ) -> Result<(), Failure> {
            self.logins.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    /// 记下所有状态；提问时按 `answer` 回答，并可以顺便发“停止”。
    struct FakeUi {
        answer: bool,
        asked: AtomicU32,
        statuses: Mutex<Vec<ScanStatus>>,
        roles: Mutex<Vec<GameRole>>,
        stop_when_asked: Option<mpsc::Sender<()>>,
    }

    impl FakeUi {
        fn new(answer: bool, stop_when_asked: Option<mpsc::Sender<()>>) -> Self {
            Self {
                answer,
                asked: AtomicU32::new(0),
                statuses: Mutex::new(Vec::new()),
                roles: Mutex::new(Vec::new()),
                stop_when_asked,
            }
        }
    }

    impl ScanUi for FakeUi {
        fn show(&self, status: &ScanStatus) {
            self.statuses.lock().unwrap().push(status.clone());
        }

        async fn ask(&self, _prompt: Prompt) -> bool {
            self.asked.fetch_add(1, Ordering::SeqCst);
            if let Some(stop) = &self.stop_when_asked {
                let _ = stop.try_send(());
            }
            self.answer
        }

        fn role_found(&self, role: &GameRole) {
            self.roles.lock().unwrap().push(role.clone());
        }
    }

    fn job() -> ScanJob {
        ScanJob {
            source: Box::new(FakeSource),
            decoder: Box::new(FakeDecoder),
            account: "账号「大号」（原神 UID 100000001，通行证 300012345）".to_owned(),
            uid: "300012345".to_owned(),
            mid: "mid_abc".to_owned(),
            stoken: SecretString::from("v2_secret_stoken"),
            idle_timeout: Duration::from_secs(300),
        }
    }

    #[tokio::test]
    async fn logs_in_after_you_say_yes() {
        let api = FakeApi::new(Ok(None));
        let ui = FakeUi::new(true, None);
        let (_stop, stop_signal) = mpsc::channel(1);
        let end = run_scan(&api, job(), &ui, stop_signal).await;
        assert!(
            matches!(&end, ScanStatus::Idle { last: Some(StopReason::LoggedIn { game, .. }) } if game == "原神"),
            "{end:?}"
        );
        assert_eq!(ui.asked.load(Ordering::SeqCst), 1);
        assert_eq!(api.logins.load(Ordering::SeqCst), 1);
        assert!(ui.statuses.lock().unwrap().contains(&ScanStatus::LoggingIn));
    }

    #[tokio::test]
    async fn declined_codes_are_not_logged_in_and_stop_ends_the_session() {
        let api = FakeApi::new(Ok(None));
        let (stop, stop_signal) = mpsc::channel(1);
        let ui = FakeUi::new(false, Some(stop));
        let end = run_scan(&api, job(), &ui, stop_signal).await;
        assert_eq!(
            end,
            ScanStatus::Idle {
                last: Some(StopReason::UserStopped)
            }
        );
        assert_eq!(api.logins.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn invalid_credential_stops_before_asking() {
        let api = FakeApi::new(Err(Failure::CredentialInvalid));
        let ui = FakeUi::new(true, None);
        let (_stop, stop_signal) = mpsc::channel(1);
        let end = run_scan(&api, job(), &ui, stop_signal).await;
        assert_eq!(
            end,
            ScanStatus::Idle {
                last: Some(StopReason::CredentialInvalid)
            }
        );
        assert_eq!(ui.asked.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn reports_the_role_found_while_checking() {
        let role = GameRole {
            nickname: "旅行者".to_owned(),
            level: 60,
            game_uid: "100000001".to_owned(),
        };
        let api = FakeApi::new(Ok(Some(role.clone())));
        let ui = FakeUi::new(true, None);
        let (_stop, stop_signal) = mpsc::channel(1);
        run_scan(&api, job(), &ui, stop_signal).await;
        assert_eq!(*ui.roles.lock().unwrap(), vec![role]);
    }
}
