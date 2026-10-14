//! 直播画面来源（设计文档第 6.5 节）：在自己的线程里取流、拉流、拆 FLV、解码，只留最新的一帧。
//! 扫描循环每次 `next_frame` 拿到的都是最新画面；识别比解码慢时，中间的帧直接丢掉。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use mhy_carry_core::capture::{CaptureError, Frame, FrameSource, SourceInfo, SourceKind};
use mhy_carry_core::flv::{FlvError, FlvParser};
use mhy_carry_core::room::LiveRoom;
use mhy_carry_core::video::{CatchUp, H264Decoder};

use crate::client::{LiveClient, LiveError};
use crate::platforms::{Platforms, StreamStatus};

/// 怎么创建解码器。系统解码器（COM）只能在创建它的线程里用，所以交给拉流线程去创建。
pub type DecoderFactory = Box<dyn Fn() -> Result<Box<dyn H264Decoder>, String> + Send>;

/// 等新画面最多等这么久；等不到就报告当前状态，扫描会话据此显示“正在连接”“等待开播”。
const WAIT_FOR_FRAME: Duration = Duration::from_secs(1);

/// 拉流线程的节奏。测试里会调短。
#[derive(Debug, Clone)]
pub(crate) struct Timing {
    /// 没开播时隔多久再查一次。
    pub offline_poll: Duration,
    /// 这么久没收到数据就当断线。
    pub stall: Duration,
    /// 连续这么多次连不上就停止。
    pub max_failures: u32,
}

impl Default for Timing {
    fn default() -> Self {
        Self {
            offline_poll: Duration::from_secs(10),
            stall: Duration::from_secs(10),
            max_failures: 5,
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
        self.shared.changed.notify_all();
    }
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
        let mut failures = 0;
        let mut next_url = 0;
        while !shared.closed() {
            let reason = match platforms.resolve(client, room).await {
                Ok(StreamStatus::Offline) => {
                    shared.set_state(State::Offline);
                    failures = 0;
                    sleep_unless_closed(timing.offline_poll, shared).await;
                    continue;
                }
                Ok(StreamStatus::Live { urls, referer }) => {
                    // 取流结果里至少有一个地址。每次重连换下一个 CDN。
                    let url = &urls[next_url % urls.len()];
                    next_url += 1;
                    let pulled = pull(url, referer, decoder, client, timing, shared).await;
                    if pulled.fatal {
                        return shared.set_state(State::Failed(pulled.reason));
                    }
                    if pulled.got_frame {
                        failures = 0;
                    }
                    pulled.reason
                }
                // 房间不存在、没有能用的流，重试也没用。
                Err(error @ (LiveError::RoomNotFound | LiveError::NoStream)) => {
                    return shared.set_state(State::Failed(error.to_string()));
                }
                Err(error) => error.to_string(),
            };
            if shared.closed() {
                return;
            }
            failures += 1;
            if failures >= timing.max_failures {
                return shared.set_state(State::Failed(reason));
            }
            shared.set_state(State::Connecting);
        }
    });
}

/// 一次拉流的结果。
struct Pulled {
    /// 中间解出过画面。连续失败的次数据此清零。
    got_frame: bool,
    reason: String,
    /// 重连也没用的错误：不是 H.264、电脑没有解码器。
    fatal: bool,
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
    let ended = |reason: String, got_frame| Pulled {
        got_frame,
        reason,
        fatal: false,
    };
    // 每次连接都用新的解码器：断线后旧解码器里的状态没用了，分辨率也可能变了。
    let mut decoder = match decoder() {
        Ok(decoder) => decoder,
        Err(reason) => {
            return Pulled {
                got_frame: false,
                reason,
                fatal: true,
            };
        }
    };
    let mut response = match client.open_stream(url, referer).await {
        Ok(response) => response,
        Err(error) => return ended(error.to_string(), false),
    };
    let mut parser = FlvParser::new();
    let mut catch_up = CatchUp::default();
    let started = Instant::now();
    let mut got_frame = false;
    while !shared.closed() {
        let chunk = match tokio::time::timeout(timing.stall, response.chunk()).await {
            Err(_) => {
                return ended(
                    format!("{} 秒没收到直播数据", timing.stall.as_secs()),
                    got_frame,
                );
            }
            Ok(Err(error)) => return ended(LiveError::network(error).to_string(), got_frame),
            Ok(Ok(None)) => return ended("直播流断开了".to_owned(), got_frame),
            Ok(Ok(Some(chunk))) => chunk,
        };
        let frames = match parser.push(&chunk) {
            Ok(frames) => frames,
            Err(error @ FlvError::NotH264) => {
                return Pulled {
                    got_frame,
                    reason: error.to_string(),
                    fatal: true,
                };
            }
            Err(error) => return ended(error.to_string(), got_frame),
        };
        for video in frames {
            let received_ms = started.elapsed().as_millis() as u64;
            if !catch_up.should_decode(video.timestamp_ms, video.key, received_ms) {
                continue;
            }
            match decoder.decode(&video.annexb, video.timestamp_ms) {
                Ok(Some(frame)) => {
                    got_frame = true;
                    shared.publish(frame);
                }
                Ok(None) => {}
                Err(message) => return ended(format!("解码失败：{message}"), got_frame),
            }
        }
    }
    ended(String::new(), got_frame)
}

/// 分小段睡，扫描结束时能尽快退出线程。
async fn sleep_unless_closed(duration: Duration, shared: &Shared) {
    let deadline = Instant::now() + duration;
    while !shared.closed() {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return;
        }
        tokio::time::sleep(left.min(Duration::from_millis(200))).await;
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

    fn timing() -> Timing {
        Timing {
            offline_poll: Duration::from_millis(50),
            stall: Duration::from_secs(2),
            max_failures: 2,
        }
    }

    fn start(server: &MockServer) -> LiveSource {
        LiveSource::start_with(
            LiveRoom::Bilibili { id: 12345 },
            fake_decoder(),
            LiveClient::for_tests(),
            Platforms {
                bilibili: Bilibili::at(&server.uri()),
            },
            timing(),
        )
        .unwrap()
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
