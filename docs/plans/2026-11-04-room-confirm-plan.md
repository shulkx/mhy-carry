# 确认直播间 实施计划

> **给执行者：** 在当前会话里按顺序执行（executing-plans），不要每个任务开一个子代理。步骤用复选框（`- [ ]`）跟踪。

**目标：** 填好直播间后，查出主播名、直播状态、分区和标题，显示在输入框下面，让你确认填对了直播间。开始扫描时，把主播名带进直播间的名称，状态和确认框里都用这个名称（设计文档第 6.5 节“确认直播间”，2026-11-04 和用户确认）。

**做法：**
- `core`：
  - `text::clip`：清理外部文本，并按字数截断；
  - `LiveRoom::label`：直播间的名称，可以带上主播名。
- `live`：
  - `LiveClient::lookup_room`：调 B 站 `getH5InfoByRoom`，3 秒内没回应就算失败；
  - `LiveSource::start` 多一个参数 `anchor`。
- 应用：
  - 新命令 `lookup_room`；
  - `scan_start` 开始直播间扫描前，自己再查一次主播名；
  - 界面在停止输入 0.5 秒后查询，丢掉过时的结果。
- 命令行：`watch --target live` 开始前打印直播间信息。

**依据：** 设计文档 [docs/design/2026-10-11-mhy-carry-design.md](../design/2026-10-11-mhy-carry-design.md) 第 6.3、6.5、9、11、16 节。不新增依赖，也不新增域名：接口仍在 `api.live.bilibili.com`。

---

## 约定

和 v0.3 计划一样：
- 命令在仓库根目录执行；
- 每个任务提交一次，提交前 fmt、clippy、测试都要通过，带上 `Cargo.lock`；
- 代码块是局部修改，按说明替换。

联网只访问公开的 B 站接口，不带凭证。

---

### 任务 1：外部文本截断和直播间名称（`core`）

**文件：** `crates/core/src/text.rs`、`crates/core/src/room.rs`

- [ ] **步骤 1：`text.rs` 在 `printable` 后面加**

```rust
/// 清理外部文本（同 [`printable`]），超过 `max` 个字时截断并加“…”。
pub fn clip(text: &str, max: usize) -> String {
    let cleaned = printable(text);
    if cleaned.chars().count() <= max {
        return cleaned;
    }
    let kept: String = cleaned.chars().take(max.saturating_sub(1)).collect();
    format!("{}…", kept.trim_end())
}
```

测试：

```rust
    #[test]
    fn clip_cleans_and_shortens() {
        assert_eq!(clip(" 憨憨的\r小喵i ", 32), "憨憨的 小喵i");
        assert_eq!(clip("一二三四五六", 6), "一二三四五六");
        assert_eq!(clip("一二三四五六七", 6), "一二三四五…");
        assert_eq!(clip("", 6), "");
    }
```

- [ ] **步骤 2：`room.rs` 给 `LiveRoom` 加**

```rust
impl LiveRoom {
    /// 直播间的名称，查到主播名时带上，比如“B 站直播间 12345（憨憨的小喵i）”
    /// （设计文档第 6.5 节“确认直播间”）。`anchor` 要先用 `text::clip` 清理过。
    pub fn label(&self, anchor: Option<&str>) -> String {
        match anchor {
            Some(anchor) if !anchor.is_empty() => format!("{self}（{anchor}）"),
            _ => self.to_string(),
        }
    }
}
```

测试：

```rust
    #[test]
    fn labels_carry_the_anchor_when_known() {
        assert_eq!(ROOM.label(Some("憨憨的小喵i")), "B 站直播间 12345（憨憨的小喵i）");
        assert_eq!(ROOM.label(Some("")), "B 站直播间 12345");
        assert_eq!(ROOM.label(None), "B 站直播间 12345");
    }
```

- [ ] **步骤 3：运行** `cargo test -p mhy-carry-core --lib -- text:: room::`，预期全部通过。
- [ ] **步骤 4：提交** `core: 外部文本截断和带主播名的直播间名称`

### 任务 2：查直播间信息（`live`）

**文件：** `crates/live/src/bilibili.rs`、`platforms.rs`、`client.rs`、`source.rs`、`lib.rs`

- [ ] **步骤 1：`platforms.rs` 加 `RoomInfo` 和 `Platforms::lookup`**

```rust
/// 直播间的主播信息（设计文档第 6.5 节“确认直播间”）。都已经清理、截断过。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoomInfo {
    /// 主播名，不为空。
    pub anchor: String,
    /// 直播中。
    pub live: bool,
    /// 分区，比如“原神”。可能为空。
    pub area: String,
    /// 直播间标题。可能为空。
    pub title: String,
}
```

`impl Platforms` 里加：

```rust
    pub async fn lookup(&self, client: &LiveClient, room: &LiveRoom) -> Result<RoomInfo, LiveError> {
        match room {
            LiveRoom::Bilibili { id } => self.bilibili.lookup(client, *id).await,
        }
    }
```

- [ ] **步骤 2：`bilibili.rs` 加 `lookup`**

常量：

```rust
/// getH5InfoByRoom 对不存在的房间返回的错误码（2026-11-04 实测）。
const INFO_NOT_FOUND: i64 = 19002000;
/// 主播名、分区和标题最多保留多少个字（设计文档第 6.5 节）。
const MAX_ANCHOR: usize = 32;
const MAX_TEXT: usize = 64;
```

`impl Bilibili` 里加：

```rust
    /// 查主播名、是否在播、分区和标题。`room` 是你输入的房间号，短号也行。
    pub async fn lookup(&self, client: &LiveClient, room: u64) -> Result<RoomInfo, LiveError> {
        let room = room.to_string();
        let info = client
            .get_json(
                self.url(
                    "/xlive/web-room/v1/index/getH5InfoByRoom",
                    &[("room_id", &room)],
                ),
                &[API_HOST],
                REFERER,
            )
            .await?;
        match info["code"].as_i64() {
            Some(0) => {}
            Some(INFO_NOT_FOUND) => return Err(LiveError::RoomNotFound),
            code => return Err(error_code(code)),
        }
        let data = &info["data"];
        let anchor = clip(
            data["anchor_info"]["base_info"]["uname"]
                .as_str()
                .unwrap_or_default(),
            MAX_ANCHOR,
        );
        if anchor.is_empty() {
            return Err(LiveError::Format(
                "getH5InfoByRoom 的返回里没有主播名".to_owned(),
            ));
        }
        let text = |value: &Value| clip(value.as_str().unwrap_or_default(), MAX_TEXT);
        Ok(RoomInfo {
            anchor,
            live: data["room_info"]["live_status"].as_i64() == Some(1),
            area: text(&data["room_info"]["area_name"]),
            title: text(&data["room_info"]["title"]),
        })
    }
```

把 `resolve` 里拼错误码的那段改成调用同一个函数：

```rust
/// 接口返回的其他错误码：可能是风控之类的临时错误，可以重试。
fn error_code(code: Option<i64>) -> LiveError {
    LiveError::Other(format!(
        "B 站直播接口返回错误码 {}",
        code.map_or("（无）".to_owned(), |code| code.to_string())
    ))
}
```

导入加 `use mhy_carry_core::text::clip;` 和 `use crate::platforms::RoomInfo;`。

测试（`bilibili.rs` 的 `tests` 里）：

```rust
    fn h5_info(body: serde_json::Value) -> ResponseTemplate {
        ResponseTemplate::new(200).set_body_json(body)
    }

    async fn lookup(server: &MockServer) -> Result<RoomInfo, LiveError> {
        Bilibili::at(&server.uri())
            .lookup(&LiveClient::for_tests(), 12345)
            .await
    }

    #[tokio::test]
    async fn looks_up_the_anchor_status_area_and_title() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/xlive/web-room/v1/index/getH5InfoByRoom"))
            .and(query_param("room_id", "12345"))
            .and(header("referer", REFERER))
            .respond_with(h5_info(json!({
                "code": 0,
                "data": {
                    "room_info": {"room_id": 32155676, "live_status": 1, "area_name": "原神", "title": "深渊\u{0007}速打"},
                    "anchor_info": {"base_info": {"uname": " 憨憨的小喵i "}}
                }
            })))
            .mount(&server)
            .await;
        assert_eq!(
            lookup(&server).await.unwrap(),
            RoomInfo {
                anchor: "憨憨的小喵i".to_owned(),
                live: true,
                area: "原神".to_owned(),
                title: "深渊 速打".to_owned(),
            }
        );
    }

    #[tokio::test]
    async fn long_texts_are_clipped_and_offline_rooms_say_so() {
        let server = MockServer::start().await;
        Mock::given(path("/xlive/web-room/v1/index/getH5InfoByRoom"))
            .respond_with(h5_info(json!({
                "code": 0,
                "data": {
                    "room_info": {"live_status": 2, "title": "长".repeat(100)},
                    "anchor_info": {"base_info": {"uname": "名".repeat(40)}}
                }
            })))
            .mount(&server)
            .await;
        let info = lookup(&server).await.unwrap();
        assert!(!info.live);
        assert_eq!(info.area, "");
        assert_eq!(info.anchor.chars().count(), 32);
        assert_eq!(info.title.chars().count(), 64);
        assert!(info.title.ends_with('…'));
    }

    #[tokio::test]
    async fn lookup_reports_missing_rooms_and_other_codes() {
        for (code, expected) in [
            (19002000, "找不到这个直播间"),
            (-352, "B 站直播接口返回错误码 -352"),
        ] {
            let server = MockServer::start().await;
            Mock::given(path("/xlive/web-room/v1/index/getH5InfoByRoom"))
                .respond_with(h5_info(json!({"code": code, "message": "x", "data": null})))
                .mount(&server)
                .await;
            let error = lookup(&server).await.unwrap_err();
            assert_eq!(error.to_string(), expected);
        }
    }

    #[tokio::test]
    async fn lookup_without_an_anchor_name_is_a_format_error() {
        let server = MockServer::start().await;
        Mock::given(path("/xlive/web-room/v1/index/getH5InfoByRoom"))
            .respond_with(h5_info(json!({"code": 0, "data": {"room_info": {}}})))
            .mount(&server)
            .await;
        assert!(matches!(lookup(&server).await, Err(LiveError::Format(_))));
    }
```

- [ ] **步骤 3：`client.rs` 加 `lookup_room`**

```rust
/// 查直播间信息最多等这么久。开始扫描时也要查，不能拖慢开始（设计文档第 6.5 节）。
const LOOKUP_LIMIT: Duration = Duration::from_secs(3);
```

`impl LiveClient` 里加：

```rust
    /// 查直播间的主播名、是否在播、分区和标题（设计文档第 6.5 节“确认直播间”）。
    pub async fn lookup_room(&self, room: &LiveRoom) -> Result<RoomInfo, LiveError> {
        tokio::time::timeout(LOOKUP_LIMIT, Platforms::official().lookup(self, room))
            .await
            .unwrap_or_else(|_| {
                Err(LiveError::Other(format!(
                    "直播平台 {} 秒内没有回应",
                    LOOKUP_LIMIT.as_secs()
                )))
            })
    }
```

导入加 `use mhy_carry_core::room::LiveRoom;` 和 `use crate::platforms::{Platforms, RoomInfo};`。

- [ ] **步骤 4：`source.rs`**

`LiveSource::start` 改成：

```rust
    /// 开始拉这个直播间的流。马上返回；取流、连接都在后台线程里做。
    /// `anchor` 是查到的主播名（已经清理过），会带进直播间的名称。
    pub fn start(
        room: LiveRoom,
        anchor: Option<&str>,
        decoder: DecoderFactory,
    ) -> Result<Self, LiveError> {
        let label = room.label(anchor);
        Self::start_with(
            room,
            label,
            decoder,
            LiveClient::new()?,
            Platforms::official(),
            Timing::default(),
        )
    }
```

`start_with` 加参数 `label: String`（放在 `room` 后面），`SourceInfo::new(SourceKind::LiveStream, &room.to_string())` 改成 `SourceInfo::new(SourceKind::LiveStream, &label)`。测试里的 `start_with` 辅助函数传 `"B 站直播间 12345".to_owned()`；联网测试改成 `LiveSource::start(LiveRoom::Bilibili { id }, None, decoder)`，再加一个联网测试：

```rust
    /// 联网测试：查一个公开直播间的主播信息。用法同上。
    #[tokio::test]
    #[ignore = "需要联网，并用 MHY_CARRY_LIVE_ROOM 指定一个直播间"]
    async fn looks_up_a_real_bilibili_room() {
        let id = std::env::var("MHY_CARRY_LIVE_ROOM")
            .expect("用 MHY_CARRY_LIVE_ROOM 指定房间号")
            .parse()
            .expect("房间号是数字");
        let info = LiveClient::new()
            .unwrap()
            .lookup_room(&LiveRoom::Bilibili { id })
            .await
            .unwrap();
        println!("{info:?}");
        assert!(!info.anchor.is_empty());
    }
```

`lib.rs` 的导出加上 `pub use platforms::RoomInfo;`，`platforms` 里的 `RoomInfo` 改成 `pub`（模块本身仍是私有的）。

- [ ] **步骤 5：调用方跟上** `crates/cli/src/watch_cmd.rs` 和 `app/src-tauri/src/commands.rs` 里的 `LiveSource::start(room, Box::new(system_decoder))` 先改成 `LiveSource::start(room, None, Box::new(system_decoder))`。
- [ ] **步骤 6：运行** `cargo test -p mhy-carry-live`，再用 `$env:MHY_CARRY_LIVE_ROOM = "<房间号>"; cargo test -p mhy-carry-live -- --ignored --nocapture looks_up` 查一个真实房间，预期打印出主播名。
- [ ] **步骤 7：提交** `live: 查直播间的主播信息`

### 任务 3：应用的命令（`app`）

**文件：** `app/src-tauri/src/commands.rs`、`main.rs`、`build.rs`、`capabilities/main.json`

- [ ] **步骤 1：`commands.rs`**

导入改成 `use mhy_carry_live::{LiveClient, LiveSource, RoomInfo};`，`use mhy_carry_core::room::{LiveRoom, parse_room};`。加：

```rust
/// 直播间的主播信息，给界面显示（设计文档第 6.5 节“确认直播间”）。
#[derive(Debug, Serialize)]
pub struct RoomView {
    pub anchor: String,
    pub live: bool,
    pub area: String,
    pub title: String,
}

/// 查直播间的主播信息，让你确认填对了直播间。返回的错误直接显示在输入框下面。
#[tauri::command]
pub async fn lookup_room(room: String) -> CommandResult<RoomView> {
    let room = parse_room(&room).map_err(|error| error.to_string())?;
    let RoomInfo {
        anchor,
        live,
        area,
        title,
    } = look_up(&room).await.map_err(|error| match error {
        LiveError::RoomNotFound => error.to_string(),
        other => format!("查不到主播信息：{other}"),
    })?;
    Ok(RoomView {
        anchor,
        live,
        area,
        title,
    })
}

async fn look_up(room: &LiveRoom) -> Result<RoomInfo, LiveError> {
    LiveClient::new()?.lookup_room(room).await
}
```

`scan_start` 里 `let job = prepare_job(` 前面加：

```rust
    // 开始扫描直播间时自己再查一次主播名，带进直播间的名称；查不到就不带（第 6.5 节）。
    let anchor = match (target, room.as_deref().map(parse_room)) {
        (TargetKind::Live, Some(Ok(live))) => look_up(&live).await.ok().map(|info| info.anchor),
        _ => None,
    };
```

`prepare_job` 加参数 `anchor: Option<&str>`（放在 `room` 后面），调用处传 `anchor.as_deref()`；直播间那一支改成 `LiveSource::start(room, anchor, Box::new(system_decoder))`。`LiveError` 从 `mhy_carry_live` 导入。

- [ ] **步骤 2：注册** `build.rs` 加 `"lookup_room",`；`main.rs` 的 `generate_handler!` 加 `commands::lookup_room,`；`capabilities/main.json` 加 `"allow-lookup-room"`。
- [ ] **步骤 3：运行** `cargo clippy -p mhy-carry --all-targets -- -D warnings`，预期通过；`permissions/autogenerated/` 多出 `lookup_room.toml`，一起提交。
- [ ] **步骤 4：** 和任务 4 一起提交。

### 任务 4：界面（`app/ui`）

**文件：** `index.html`、`style.css`、`main.js`

- [ ] **步骤 1：`index.html`** 在 `live-pick` 那个 `div` 后面加：

```html
    <div class="room-info" id="room-info" aria-live="polite" hidden>
      <div class="room-anchor" id="room-anchor"></div>
      <div class="room-title" id="room-title"></div>
    </div>
```

- [ ] **步骤 2：`style.css`** 在“开始扫描”那组样式前面加：

```css
/* 直播间的主播信息（设计文档第 6.5 节“确认直播间”）。 */
.room-info { font-size: 13px; color: var(--muted); margin-top: -4px; min-width: 0; }
.room-info .room-anchor { color: var(--ink); overflow-wrap: anywhere; }
.room-info.bad .room-anchor { color: var(--bad); }
.room-info .room-title { white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
```

- [ ] **步骤 3：`main.js`**

在“扫描”一节前面加：

```js
// ---- 直播间信息（设计文档第 6.5 节“确认直播间”） ----

// 最近一次查询的结果：empty | loading | ok | error。
let roomInfo = { kind: "empty" };
// 最近一次查询的序号和输入。回来的结果不是最近一次的就丢掉。
let roomQuery = 0;
let roomQueried = null;
let roomTimer = null;

function scheduleRoomLookup() {
  clearTimeout(roomTimer);
  roomTimer = setTimeout(lookupRoom, 500);
}

async function lookupRoom() {
  clearTimeout(roomTimer);
  const text = $("live-room").value.trim();
  // 同一个输入已经查到或正在查，就不再查；上次出错的可以再查。
  if (text === roomQueried && (roomInfo.kind === "ok" || roomInfo.kind === "loading")) return;
  roomQueried = text;
  const seq = ++roomQuery;
  roomInfo = text ? { kind: "loading" } : { kind: "empty" };
  renderRoomInfo();
  if (!text) return;
  let next;
  try {
    next = { kind: "ok", info: await invoke("lookup_room", { room: text }) };
  } catch (error) {
    next = { kind: "error", message: String(error) };
  }
  if (seq !== roomQuery) return;
  roomInfo = next;
  renderRoomInfo();
}

function renderRoomInfo() {
  const box = $("room-info");
  box.hidden = currentTarget() !== "live" || roomInfo.kind === "empty";
  box.classList.toggle("bad", roomInfo.kind === "error");
  let first = "";
  let second = "";
  if (roomInfo.kind === "loading") first = "正在查询直播间…";
  if (roomInfo.kind === "error") first = roomInfo.message;
  if (roomInfo.kind === "ok") {
    const info = roomInfo.info;
    first = ["主播：" + info.anchor, info.live ? "直播中" : "未开播", info.area].filter(Boolean).join(" · ");
    second = info.title;
  }
  $("room-anchor").textContent = first;
  $("room-title").textContent = second;
  $("room-title").hidden = !second;
}
```

`renderScan` 末尾加 `renderRoomInfo();`。

`$("live-room")` 的 `input` 处理改成：

```js
$("live-room").addEventListener("input", () => {
  $("live-room").dataset.edited = "1";
  scheduleRoomLookup();
});
```

截图目标的点击处理里，`loadTargets(name);` 后面加 `if (name === "live") lookupRoom();`。`main()` 里 `loadTargets(currentTarget());` 后面加 `if (currentTarget() === "live") lookupRoom();`。

- [ ] **步骤 4：检查** `node --check app/ui/main.js`。
- [ ] **步骤 5：远程调试检查**（临时数据目录、假账号，查的是公开直播间）：
  - 选“直播间”，填一个在播房间：先显示“正在查询直播间…”，再显示主播名、直播中、分区、标题；
  - 填 `abc`：红字“认不出这个直播间…”；
  - 填 `999999999999`：红字“找不到这个直播间”；
  - 清空：信息行消失；
  - 快速连续输入两个房间号，只显示后一个的结果；
  - 重启应用，记着上次的直播间时自动查一次；
  - 深色主题截图，标题太长时截断。
- [ ] **步骤 6：提交** `app: 填好直播间后显示主播信息`

### 任务 5：命令行（`cli`）

**文件：** `crates/cli/src/watch_cmd.rs`

- [ ] **步骤 1：** `run` 里 `let source = choose_source(…)` 换成：

```rust
    let target = target.unwrap_or(ctx.settings.scan.default_target);
    let anchor = match target {
        TargetKind::Live => describe_room(&room).await,
        _ => None,
    };
    let source = choose_source(target, &room, anchor.as_deref())?;
```

`choose_source` 加参数 `anchor: Option<&str>`，直播间那一支改成 `LiveSource::start(room, anchor, Box::new(system_decoder))`。加：

```rust
/// 开始前查一下直播间的主播信息，让你确认填对了（设计文档第 6.5 节“确认直播间”）。
/// 返回主播名；查不到时打印原因，照常扫描。认不出的输入留给 `choose_source` 报错。
async fn describe_room(room: &str) -> Option<String> {
    let live = parse_room(room).ok()?;
    let looked = match LiveClient::new() {
        Ok(client) => client.lookup_room(&live).await,
        Err(error) => Err(error),
    };
    match looked {
        Ok(info) => {
            let status = if info.live { "直播中" } else { "未开播" };
            let area = if info.area.is_empty() {
                String::new()
            } else {
                format!(" · {}", info.area)
            };
            println!("{}：主播 {} · {status}{area}", live, info.anchor);
            if !info.title.is_empty() {
                println!("  {}", info.title);
            }
            Some(info.anchor)
        }
        Err(error) => {
            println!("查不到主播信息：{error}");
            None
        }
    }
}
```

导入改成 `use mhy_carry_live::{LiveClient, LiveSource};`。

- [ ] **步骤 2：运行** `cargo clippy -p mhy-carry-cli --all-targets -- -D warnings`。
- [ ] **步骤 3：提交** `cli: watch 开始前显示直播间的主播信息`

### 任务 6：文档、审查、发布

- [ ] README 的“直播间抢码”一节加一句：填好直播间后，输入框下面会显示主播名、是否在播、分区和标题，先核对是不是你要的直播间。
- [ ] CLAUDE.md 的进度写上这次的计划。
- [ ] 提交 `docs: 确认直播间的使用说明`，推送，等 CI。
- [ ] 派一个子代理只读审查这批改动，修完再继续。
- [ ] 发布前先问用户。

---

## 执行记录

- **任务 4：** 远程调试时发现，改了输入后的 0.5 秒里，下面还显示着上一个房间的主播，容易看错。`scheduleRoomLookup` 因此改成 `roomInputChanged`：输入一改，旧结果马上作废，先显示“正在查询直播间…”，停止输入 0.5 秒后再查。
- **CI：** 推送后 `cargo deny` 失败，是这段时间新出的安全公告，和这次改动无关。
  - `rustls-webpki` 升到 0.103.16（RUSTSEC-2026-0382，证书校验），`tokio` 升到 1.53.3（RUSTSEC-2026-0386）。两个版本都已经发布两周以上。
  - `zvariant`、`zbus_names`、`zvariant_derive` 不再维护（RUSTSEC-2026-0370、0371、0372）。它们只在 Linux 上由截图库 xcap 引入，Windows 上不编译，所以加进 `deny.toml` 的忽略列表，写明原因。
- **任务 6 的独立审查**：没有安全问题；1 个中等问题、几个低优先级问题，改动如下：
  - **中等（已修）：** `scan_start` 查主播名的最多 3 秒发生在占用扫描状态之后。这几秒里界面看起来是空闲的，后端却当作扫描中：再点开始会被悄悄忽略，改输入后显示的是另一个房间的主播。
    - 后端：查询挪到占用扫描状态之前。
    - 界面：点开始后到后端回应之前，按钮显示“正在开始…”，目标、输入框和自动确认都不能改。
  - **低（已修）：**
    - 防抖计时器可能在开始扫描之后才触发。现在点开始时，先把还没发出的查询发出去。
    - 在设置里把默认目标改成“直播间”时不会去查。
    - 查到的“直播中/未开播”会一直沿用，可能过时。
    - 后两条一起改成：`renderScan` 发现目标切到直播间、或者一次扫描刚结束时就重查；去重只对正在查的请求。
    - `clip(…, 0)` 返回了“…”，现在返回空串。
  - **测试缺口（已补）：** 分区的截断；查询超时（抽出 `lookup_with`，可以指定接口地址和时限）。
  - **没改：**
    - 每次查询都新建客户端，审查说是可选的优化；
    - 主播名会进入确认框，这是设计要求，主播名已经清理并限长。
