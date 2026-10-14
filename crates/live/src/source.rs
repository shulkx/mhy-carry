//! 直播画面来源（设计文档第 6.5 节）：在自己的线程里取流、拉流、拆 FLV、解码，只留最新的一帧。
//! 扫描循环每次 `next_frame` 拿到的都是最新画面；识别比解码慢时，中间的帧直接丢掉。

use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use mhy_carry_core::capture::{CaptureError, Frame, FrameSource, SourceInfo, SourceKind};
use mhy_carry_core::flv::{FlvError, FlvParser};
use mhy_carry_core::room::LiveRoom;
use mhy_carry_core::video::{CatchUp, H264Decoder};
use tokio::sync::Notify;

use crate::client::{LiveClient, LiveError};
use crate::platforms::{Platforms, StreamStatus};

/// 怎么创建解码器。系统解码器（COM）只能在创建它的线程里用，所以交给拉流线程去创建。
pub type DecoderFactory = Box<dyn Fn() -> Result<Box<dyn H264Decoder>, String> + Send>;

/// 等新画面最多等这么久；等不到就报告当前状态，扫描会话据此显示“正在连接”“等待开播”。
const WAIT_FOR_FRAME: Duration = Duration::from_secs(1);

/// 拉流线程的节奏（设计文档第 6.5 节）。测试里会调短。
#[derive(Debug, Clone)]
pub(crate) struct Timing {
    /// 没开播时隔多久再查一次。
    pub offline_poll: Duration,
    /// 打开流地址后这么久没有响应、或者拉流中这么久没收到数据，就当断线。
    pub stall: Duration,
    /// 连不上时，第一次重试前等多久；之后每次翻倍，最多等 `retry_max`。
    pub retry_first: Duration,
    pub retry_max: Duration,
    /// 解出画面后又拉了这么久才断的，算正常断线：立刻重连，重新计数。
    pub stable: Duration,
    /// 连续连不上至少这么多次、并且持续了 `give_up_after` 以上，才停止。
    pub give_up_failures: u32,
    pub give_up_after: Duration,
}

impl Default for Timing {
    fn default() -> Self {
        Self {
            offline_poll: Duration::from_secs(10),
            stall: Duration::from_secs(10),
            retry_first: Duration::from_secs(1),
            retry_max: Duration::from_secs(30),
            stable: Duration::from_secs(30),
            give_up_failures: 5,
            give_up_after: Duration::from_secs(60),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum State {
    Connecting,
    Offline,
    Live,
    Failed(String),
}

struct Slot {
    state: State,
    frame: Option<Frame>,
}

struct Shared {
    slot: Mutex<Slot>,
    changed: Condvar,
    /// 扫描结束，拉流线程该退出了。
    closed: AtomicBool,
    /// 扫描结束时叫醒拉流线程，不用等网络请求超时。
    wake: Notify,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, Slot> {
        self.slot
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn set_state(&self, state: State) {
        self.lock().state = state;
        self.changed.notify_all();
    }

    /// 放进最新的一帧，旧的没被取走就直接覆盖。
    fn publish(&self, frame: Frame) {
        let mut slot = self.lock();
        slot.frame = Some(frame);
        slot.state = State::Live;
        drop(slot);
        self.changed.notify_all();
    }

    fn closed(&self) -> bool {
        self.closed.load(Ordering::Relaxed)
    }

    /// 等 `future` 完成；扫描结束时马上返回 None。
    async fn unless_closed<F: Future>(&self, future: F) -> Option<F::Output> {
        if self.closed() {
            return None;
        }
        tokio::select! {
            output = future => Some(output),
            () = self.wake.notified() => None,
        }
    }

    /// 睡一会儿；扫描结束时马上返回 false。
    async fn sleep(&self, duration: Duration) -> bool {
        self.unless_closed(tokio::time::sleep(duration))
            .await
            .is_some()
    }
}

/// 直播画面来源。丢弃时通知拉流线程退出。
pub struct LiveSource {
    shared: Arc<Shared>,
    info: SourceInfo,
}

impl LiveSource {
    /// 开始拉这个直播间的流。马上返回；取流、连接都在后台线程里做。
    pub fn start(room: LiveRoom, decoder: DecoderFactory) -> Result<Self, LiveError> {
        Self::start_with(
            room,
            decoder,
            LiveClient::new()?,
            Platforms::official(),
            Timing::default(),
        )
    }

    pub(crate) fn start_with(
        room: LiveRoom,
        decoder: DecoderFactory,
        client: LiveClient,
        platforms: Platforms,
        timing: Timing,
    ) -> Result<Self, LiveError> {
        let info = SourceInfo::new(SourceKind::LiveStream, &room.to_string());
        let shared = Arc::new(Shared {
            slot: Mutex::new(Slot {
                state: State::Connecting,
                frame: None,
            }),
            changed: Condvar::new(),
            closed: AtomicBool::new(false),
            wake: Notify::new(),
        });
        let worker = Arc::clone(&shared);
        std::thread::Builder::new()
            .name("live-stream".to_owned())
            .spawn(move || run(&room, &decoder, &client, &platforms, &timing, &worker))
            .map_err(|error| LiveError::Other(format!("启动拉流线程失败：{error}")))?;
        Ok(Self { shared, info })
    }
}

impl FrameSource for LiveSource {
    fn next_frame(&mut self) -> Result<Frame, CaptureError> {
        let slot = self.shared.lock();
        let (mut slot, _) = self
            .shared
            .changed
            .wait_timeout_while(slot, WAIT_FOR_FRAME, |slot| {
                slot.frame.is_none() && !matches!(slot.state, State::Failed(_))
            })
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(frame) = slot.frame.take() {
            return Ok(frame);
        }
        Err(match &slot.state {
            State::Offline => CaptureError::Offline,
            State::Failed(message) => CaptureError::LiveFailed(message.clone()),
            // 在播却一秒没有新画面，多半是卡住了，也算正在连接。
            State::Connecting | State::Live => CaptureError::Connecting,
        })
    }

    fn describe(&self) -> SourceInfo {
        self.info.clone()
    }
}

impl Drop for LiveSource {
    fn drop(&mut self) {
        self.shared.closed.store(true, Ordering::Relaxed);
        // notify_one 会留下一个许可：拉流线程这时不在等，下次一等就会被叫醒。
        self.shared.wake.notify_one();
        self.shared.changed.notify_all();
    }
}

/// 连续连不上的记录。
#[derive(Default)]
struct Failures {
    count: u32,
    /// 这一串失败里第一次失败的时间。
    since: Option<Instant>,
}

/// 拉流线程：取流 → 拉流 → 断了就重连，直到扫描结束或者彻底失败。
fn run(
    room: &LiveRoom,
    decoder: &DecoderFactory,
    client: &LiveClient,
    platforms: &Platforms,
    timing: &Timing,
    shared: &Shared,
) {
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            return shared.set_state(State::Failed(format!("启动拉流线程失败：{error}")));
        }
    };
    runtime.block_on(async {
        let mut failures = Failures::default();
        let mut next_url = 0;
        loop {
            let Some(resolved) = shared.unless_closed(platforms.resolve(client, room)).await else {
                return;
            };
            let reason = match resolved {
                Ok(StreamStatus::Offline) => {
                    shared.set_state(State::Offline);
                    failures = Failures::default();
                    if !shared.sleep(timing.offline_poll).await {
                        return;
                    }
                    continue;
                }
                Ok(StreamStatus::Live { urls, referer }) => {
                    shared.set_state(State::Connecting);
                    // 取流结果里至少有一个地址。每次重连换下一个 CDN。
                    let url = &urls[next_url % urls.len()];
                    next_url += 1;
                    match pull(url, referer, decoder, client, timing, shared).await {
                        Pulled::Closed => return,
                        Pulled::Fatal(reason) => return shared.set_state(State::Failed(reason)),
                        // 正常拉了一段时间才断：立刻重连，重新计数。
                        Pulled::Dropped { stable: true, .. } => {
                            failures = Failures::default();
                            continue;
                        }
                        Pulled::Dropped { reason, .. } => reason,
                    }
                }
                // 房间不存在、没有能用的流，重试也没用。
                Err(error @ (LiveError::RoomNotFound | LiveError::NoStream)) => {
                    return shared.set_state(State::Failed(error.to_string()));
                }
                Err(error) => error.to_string(),
            };
            // 连不上：持续一段时间才停止，中间等一会儿再试。免得网络闪断一下就停，
            // 也免得一直连不上时不停地请求（设计文档第 6.5 节）。
            let since = *failures.since.get_or_insert_with(Instant::now);
            failures.count += 1;
            if failures.count >= timing.give_up_failures && since.elapsed() >= timing.give_up_after
            {
                return shared.set_state(State::Failed(reason));
            }
            shared.set_state(State::Connecting);
            if !shared.sleep(retry_wait(timing, failures.count)).await {
                return;
            }
        }
    });
}

/// 第 `count` 次连不上之后等多久：从 `retry_first` 起每次翻倍，最多 `retry_max`。
fn retry_wait(timing: &Timing, count: u32) -> Duration {
    let doublings = count.saturating_sub(1).min(16);
    timing
        .retry_first
        .saturating_mul(1 << doublings)
        .min(timing.retry_max)
}

/// 一次拉流怎么结束的。
enum Pulled {
    /// 扫描结束了。
    Closed,
    /// 重连也没用：不是 H.264、电脑没有解码器。
    Fatal(String),
    /// 断线或出错，可以重连。`stable` 表示之前正常拉了一段时间。
    Dropped { reason: String, stable: bool },
}

/// 拉一个流地址，直到断线、出错或者扫描结束。
async fn pull(
    url: &str,
    referer: &'static str,
    decoder: &DecoderFactory,
    client: &LiveClient,
    timing: &Timing,
    shared: &Shared,
) -> Pulled {
    // 每次连接都用新的解码器：断线后旧解码器里的状态没用了，分辨率也可能变了。
    let mut decoder = match decoder() {
        Ok(decoder) => decoder,
        Err(reason) => return Pulled::Fatal(reason),
    };
    // 第一次解出画面的时间。从这时起拉够 `stable` 才断的，算正常断线。
    let mut first_frame: Option<Instant> = None;
    let dropped = |reason: String, first_frame: Option<Instant>| Pulled::Dropped {
        reason,
        stable: first_frame.is_some_and(|first| first.elapsed() >= timing.stable),
    };
    // 服务器接受了连接却一直不回应时，也按断线处理。
    let opened = shared
        .unless_closed(tokio::time::timeout(
            timing.stall,
            client.open_stream(url, referer),
        ))
        .await;
    let mut response = match opened {
        None => return Pulled::Closed,
        Some(Err(_)) => return dropped("直播服务器没有响应".to_owned(), None),
        Some(Ok(Err(error))) => return dropped(error.to_string(), None),
        Some(Ok(Ok(response))) => response,
    };
    let mut parser = FlvParser::new();
    let mut catch_up = CatchUp::default();
    let started = Instant::now();
    loop {
        let read = shared
            .unless_closed(tokio::time::timeout(timing.stall, response.chunk()))
            .await;
        let chunk = match read {
            None => return Pulled::Closed,
            Some(Err(_)) => {
                let reason = format!("{} 秒没收到直播数据", timing.stall.as_secs());
                return dropped(reason, first_frame);
            }
            Some(Ok(Err(error))) => {
                return dropped(LiveError::network(error).to_string(), first_frame);
            }
            Some(Ok(Ok(None))) => return dropped("直播流断开了".to_owned(), first_frame),
            Some(Ok(Ok(Some(chunk)))) => chunk,
        };
        let frames = match parser.push(&chunk) {
            Ok(frames) => frames,
            Err(error @ FlvError::NotH264) => return Pulled::Fatal(error.to_string()),
            Err(error) => return dropped(error.to_string(), first_frame),
        };
        for video in frames {
            let received_ms = started.elapsed().as_millis() as u64;
            if !catch_up.should_decode(video.timestamp_ms, video.key, received_ms) {
                continue;
            }
            match decoder.decode(&video.annexb, video.timestamp_ms) {
                Ok(Some(frame)) => {
                    first_frame.get_or_insert_with(Instant::now);
                    shared.publish(frame);
                }
                Ok(None) => {}
                Err(message) => return dropped(format!("解码失败：{message}"), first_frame),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bilibili::Bilibili;
    use crate::bilibili::tests::mount_room;
    use wiremock::matchers::path;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// 假解码器：关键帧（数据里带 SPS）解出一帧，宽度是数据长度；其他帧不出画面。
    struct FakeDecoder;

    impl H264Decoder for FakeDecoder {
        fn decode(&mut self, annexb: &[u8], _timestamp_ms: u32) -> Result<Option<Frame>, String> {
            let key = annexb.windows(5).any(|w| w == [0, 0, 0, 1, 0x67]);
            Ok(key.then(|| Frame {
                width: annexb.len() as u32,
                height: 1,
                luma: vec![0; annexb.len()],
            }))
        }
    }

    fn fake_decoder() -> DecoderFactory {
        Box::new(|| Ok(Box::new(FakeDecoder) as Box<dyn H264Decoder>))
    }

    fn tag(kind: u8, timestamp: u32, body: &[u8]) -> Vec<u8> {
        let size = body.len() as u32;
        let mut tag = vec![kind];
        tag.extend_from_slice(&size.to_be_bytes()[1..]);
        tag.extend_from_slice(&timestamp.to_be_bytes()[1..]);
        tag.push(0);
        tag.extend_from_slice(&[0, 0, 0]);
        tag.extend_from_slice(body);
        tag.extend_from_slice(&(11 + size).to_be_bytes());
        tag
    }

    /// 一段 H.264 的 FLV：配置、一个关键帧、一个普通帧。`first` 是视频标签的第一个字节。
    fn flv(first: u8) -> Vec<u8> {
        let mut bytes = b"FLV\x01\x01\x00\x00\x00\x09\x00\x00\x00\x00".to_vec();
        let config = [
            first, 0, 0, 0, 0, 1, 0x64, 0, 0x1f, 0xff, 0xe1, 0, 2, 0x67, 0x64, 1, 0, 2, 0x68, 0xee,
        ];
        bytes.extend(tag(9, 0, &config));
        bytes.extend(tag(9, 40, &[first, 1, 0, 0, 0, 0, 0, 0, 2, 0x65, 0x88]));
        bytes.extend(tag(
            9,
            73,
            &[(first & 0x0f) | 0x20, 1, 0, 0, 0, 0, 0, 0, 2, 0x41, 0x9a],
        ));
        bytes
    }

    /// 测试用的节奏：重试只等几十毫秒，连不上 2 次就停止。
    fn timing() -> Timing {
        Timing {
            offline_poll: Duration::from_millis(50),
            stall: Duration::from_secs(2),
            retry_first: Duration::from_millis(20),
            retry_max: Duration::from_millis(50),
            stable: Duration::from_secs(3600),
            give_up_failures: 2,
            give_up_after: Duration::ZERO,
        }
    }

    fn start(server: &MockServer) -> LiveSource {
        start_with(server, fake_decoder(), timing())
    }

    fn start_with(server: &MockServer, decoder: DecoderFactory, timing: Timing) -> LiveSource {
        LiveSource::start_with(
            LiveRoom::Bilibili { id: 12345 },
            decoder,
            LiveClient::for_tests(),
            Platforms {
                bilibili: Bilibili::at(&server.uri()),
            },
            timing,
        )
        .unwrap()
    }

    /// 流地址被请求了几次。
    async fn stream_requests(server: &MockServer) -> usize {
        let requests = server.received_requests().await.unwrap_or_default();
        requests
            .iter()
            .filter(|request| request.url.path() == "/live/stream.flv")
            .count()
    }

    /// next_frame 会阻塞，放到阻塞线程里调用，免得卡住假服务器。
    async fn next(source: LiveSource) -> (LiveSource, Result<Frame, CaptureError>) {
        tokio::task::spawn_blocking(move || {
            let mut source = source;
            let result = source.next_frame();
            (source, result)
        })
        .await
        .unwrap()
    }

    /// 一直取，直到拿到不是“正在连接”的结果，最多试 10 次。
    async fn settle(mut source: LiveSource) -> (LiveSource, Result<Frame, CaptureError>) {
        for _ in 0..10 {
            let (back, result) = next(source).await;
            source = back;
            if result != Err(CaptureError::Connecting) {
                return (source, result);
            }
        }
        (source, Err(CaptureError::Connecting))
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn live_rooms_produce_frames() {
        let server = MockServer::start().await;
        mount_room(&server, true, &[server.uri()]).await;
        Mock::given(path("/live/stream.flv"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(flv(0x17)))
            .mount(&server)
            .await;
        let source = start(&server);
        assert_eq!(source.describe().kind, SourceKind::LiveStream);
        assert_eq!(source.describe().label, "B 站直播间 12345");
        let (_source, result) = settle(source).await;
        assert!(matches!(result, Ok(Frame { height: 1, .. })), "{result:?}");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn offline_rooms_wait() {
        let server = MockServer::start().await;
        mount_room(&server, false, &[]).await;
        let (_source, result) = settle(start(&server)).await;
        assert_eq!(result, Err(CaptureError::Offline));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn unknown_rooms_stop() {
        let server = MockServer::start().await;
        Mock::given(path("/room/v1/Room/room_init"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({"code": 60004, "data": null})),
            )
            .mount(&server)
            .await;
        let (_source, result) = settle(start(&server)).await;
        assert_eq!(
            result,
            Err(CaptureError::LiveFailed("找不到这个直播间".to_owned()))
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn repeated_failures_stop() {
        let server = MockServer::start().await;
        mount_room(&server, true, &[server.uri()]).await;
        Mock::given(path("/live/stream.flv"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;
        let (_source, result) = settle(start(&server)).await;
        assert_eq!(
            result,
            Err(CaptureError::LiveFailed("直播平台返回 HTTP 404".to_owned()))
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn servers_that_never_answer_count_as_failures() {
        let server = MockServer::start().await;
        mount_room(&server, true, &[server.uri()]).await;
        Mock::given(path("/live/stream.flv"))
            .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(30)))
            .mount(&server)
            .await;
        let timing = Timing {
            stall: Duration::from_millis(300),
            ..timing()
        };
        let (_source, result) = settle(start_with(&server, fake_decoder(), timing)).await;
        assert_eq!(
            result,
            Err(CaptureError::LiveFailed("直播服务器没有响应".to_owned()))
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn failures_are_retried_until_the_time_limit() {
        let server = MockServer::start().await;
        mount_room(&server, true, &[server.uri()]).await;
        Mock::given(path("/live/stream.flv"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;
        let timing = Timing {
            give_up_after: Duration::from_millis(600),
            ..timing()
        };
        let (_source, result) = settle(start_with(&server, fake_decoder(), timing)).await;
        assert_eq!(
            result,
            Err(CaptureError::LiveFailed("直播平台返回 HTTP 404".to_owned()))
        );
        // 每次最多等 50 毫秒，600 毫秒里远不止试 2 次。
        assert!(stream_requests(&server).await > 4);
    }

    #[test]
    fn retries_wait_longer_each_time() {
        let timing = Timing::default();
        let waits: Vec<u64> = (1..=7)
            .map(|count| retry_wait(&timing, count).as_secs())
            .collect();
        assert_eq!(waits, [1, 2, 4, 8, 16, 30, 30]);
        assert_eq!(retry_wait(&timing, u32::MAX), Duration::from_secs(30));
    }

    /// 只记下自己有没有被丢弃的解码器。
    struct WatchedDecoder(Arc<AtomicBool>);

    impl H264Decoder for WatchedDecoder {
        fn decode(&mut self, _annexb: &[u8], _timestamp_ms: u32) -> Result<Option<Frame>, String> {
            Ok(None)
        }
    }

    impl Drop for WatchedDecoder {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn stopping_ends_the_worker_right_away() {
        // 服务器 30 秒后才回应。扫描结束时拉流线程不能等到那时才退出。
        let server = MockServer::start().await;
        mount_room(&server, true, &[server.uri()]).await;
        Mock::given(path("/live/stream.flv"))
            .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(30)))
            .mount(&server)
            .await;
        let dropped = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&dropped);
        let decoder: DecoderFactory = Box::new(move || {
            Ok(Box::new(WatchedDecoder(Arc::clone(&flag))) as Box<dyn H264Decoder>)
        });
        let timing = Timing {
            stall: Duration::from_secs(60),
            ..timing()
        };
        let source = start_with(&server, decoder, timing);
        for _ in 0..50 {
            if stream_requests(&server).await > 0 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert_eq!(stream_requests(&server).await, 1);
        drop(source);
        for _ in 0..50 {
            if dropped.load(Ordering::SeqCst) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!("扫描结束 1 秒后拉流线程还没退出");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn non_h264_streams_stop() {
        let server = MockServer::start().await;
        mount_room(&server, true, &[server.uri()]).await;
        Mock::given(path("/live/stream.flv"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(flv(0x1c)))
            .mount(&server)
            .await;
        let (_source, result) = settle(start(&server)).await;
        assert_eq!(
            result,
            Err(CaptureError::LiveFailed("直播流不是 H.264".to_owned()))
        );
    }

    /// 联网测试：拉一个正在直播的 B 站直播间（公开直播，不带凭证），用系统解码器解码、
    /// 用 rxing 识别，模拟扫描循环跑 5 秒。
    /// 用法：`$env:MHY_CARRY_LIVE_ROOM = "房间号"; cargo test -p mhy-carry-live -- --ignored --nocapture`
    #[cfg(windows)]
    #[test]
    #[ignore = "需要联网，并用 MHY_CARRY_LIVE_ROOM 指定一个正在直播的房间"]
    fn pulls_a_real_bilibili_room() {
        use mhy_carry_core::decode::{QrDecoder, RxingDecoder};
        use mhy_carry_platform::video::MfH264Decoder;

        let id = std::env::var("MHY_CARRY_LIVE_ROOM")
            .expect("用 MHY_CARRY_LIVE_ROOM 指定房间号")
            .parse()
            .expect("房间号是数字");
        let decoder: DecoderFactory = Box::new(|| {
            MfH264Decoder::new().map(|decoder| Box::new(decoder) as Box<dyn H264Decoder>)
        });
        let started = Instant::now();
        let mut source = LiveSource::start(LiveRoom::Bilibili { id }, decoder).unwrap();
        let mut first = None;
        let mut scanned = 0;
        let mut size = (0, 0);
        while started.elapsed() < Duration::from_secs(30) {
            match source.next_frame() {
                Ok(frame) => {
                    let first = *first.get_or_insert(started.elapsed());
                    size = (frame.width, frame.height);
                    RxingDecoder.decode(frame);
                    scanned += 1;
                    if started.elapsed() > first + Duration::from_secs(5) {
                        break;
                    }
                }
                Err(CaptureError::Connecting) => {}
                Err(other) => panic!("{other}"),
            }
        }
        let first = first.expect("30 秒内没解出画面");
        println!(
            "第一帧用了 {} 毫秒；之后 5 秒解码并识别了 {scanned} 帧，画面 {}×{}",
            first.as_millis(),
            size.0,
            size.1
        );
        assert!(scanned > 10, "{scanned}");
    }
}
