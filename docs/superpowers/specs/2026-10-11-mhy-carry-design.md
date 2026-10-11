# MhyCarry 设计文档（v1）

- 日期：2026-10-11
- 状态：待审阅
- v1 范围：Windows，原神国服官服，一个账号

## 1. 背景与目标

MhyCarry 识别屏幕上的米哈游游戏登录二维码，用你绑定的账号确认登录，扫码前不用掏出手机。常见用法是让朋友或主播帮你打难关：对方的游戏客户端显示登录二维码，你在自己电脑上用 MhyCarry 扫码，把账号登录到对方那里。

同类工具 MHY_Scanner 的 Release 安装包不是由 CI 从公开源码构建的，也无法和源码对照验证。程序启动时还会访问一个归属不明的第三方服务器。MhyCarry 的出发点就是**可验证、不经过第三方**：
- 账号凭证只发给米哈游官方域名；
- 发布的程序只由 CI 从公开源码构建，并附构建来源证明；
- 依赖尽量少，代码容易审查。

## 2. 范围

**v1 包含：**
- 用米游社 App 扫码绑定一个账号，可以解绑。
- 扫描三种截图目标里的原神登录二维码：自动识别的游戏窗口、手动指定的窗口（比如浏览器里的云原神）、整个显示器。
- 每次登录前都弹出系统原生对话框，由你确认。
- 只发布 Windows 版，包括安装包和免安装 exe。

**v1 不包含：**
- 加密存储（v1 明文存储，见第 8 节）、多账号界面、托盘图标、自动更新。
- 其他游戏、直播流抢码、macOS 和 Linux 版。
- 短信登录、Cookie 登录、密码登录。

**以后的扩展**都已经在架构里留好了位置（第 5 节）：
- 其他游戏：加一条游戏配置。
- 直播流抢码：加一个画面来源和一种确认策略。
- 加密存储：换一个存储实现。
- macOS 和 Linux：加对应平台的截图实现。

## 3. 技术选型

| 用途 | 选择 | 说明 |
|---|---|---|
| 语言 | Rust（edition 2024） | 在 M0 时取当时最新的 stable 版本，写进 `rust-toolchain.toml` 固定下来 |
| 桌面应用框架 | Tauri 2 | 使用系统 WebView（Windows 上是 WebView2） |
| 前端 | 纯 HTML/CSS/JS | 不引入 npm 依赖，没有构建步骤。不用 TypeScript，因为编译 TS 需要 npm 工具链 |
| Tauri CLI | `cargo install tauri-cli --locked` | 整个工具链只需要 cargo |
| HTTP | reqwest + rustls | 异步，运行在 tokio 上 |
| JSON | serde / serde_json | |
| URL 解析 | url | |
| 二维码识别 | rxing | 纯 Rust 移植的 ZXing，不需要 C++ 工具链 |
| 二维码生成 | qrcode | 绑定账号时生成 SVG 显示在界面上；测试里也用它造样例 |
| 截图 | xcap 或 windows-capture | 在 M1 实测后二选一（第 14 节） |
| 窗口枚举 | windows（微软官方 crate） | |
| 系统对话框 | tauri-plugin-dialog（只从 Rust 端调用） | |
| 敏感值包装 | secrecy | 打印时自动显示成 `[REDACTED]` |
| 错误类型 | thiserror | |
| 日志 | tracing | |
| 命令行 | clap | |
| 测试 | proptest、wiremock、qrcode | |

## 4. 仓库结构

仓库目录 `E:\GitHub\mhy-carry`，GitHub 仓库名 `shulkx/mhy-carry`。

```
mhy-carry/
├─ Cargo.toml              workspace
├─ rust-toolchain.toml
├─ deny.toml               cargo-deny 配置
├─ crates/
│  ├─ core/                mhy-carry-core：纯逻辑，不访问网络，不调用操作系统 API
│  ├─ api/                 mhy-carry-api：米哈游接口客户端
│  ├─ platform/            mhy-carry-platform：窗口枚举、截图、数据目录
│  └─ cli/                 mhy-carry-cli：命令行工具，用来验证协议和调试
├─ app/
│  ├─ src-tauri/           mhy-carry：Tauri 应用（Rust 端）
│  └─ ui/                  index.html、main.js、style.css
├─ docs/
└─ .github/workflows/      ci.yml、release.yml
```

依赖关系是单向的：`app`、`cli` 依赖 `api`、`platform`，它们再依赖 `core`。`core` 不依赖其他任何单元，也不依赖任何平台相关的库。

命名约定：
- 仓库、目录和 Cargo 包名用小写加连字符（`mhy-carry-core`）。
- 给人看的名字用 `MhyCarry`：窗口标题、安装包、README 标题、数据目录。
- Tauri identifier 用 `io.github.shulkx.mhycarry`。

## 5. 核心接口（扩展点）

下面的接口都定义在 `core` 里，签名只是示意，实现时以代码为准。

**`FrameSource`（画面来源）：** 提供一帧帧画面，并说明画面来自哪里。

```rust
pub trait FrameSource {
    fn next_frame(&mut self) -> Result<Frame, CaptureError>;
    fn describe(&self) -> SourceInfo;
}
pub struct Frame { pub width: u32, pub height: u32, pub luma: Vec<u8> } // 灰度图
pub enum SourceKind { GameWindow, PickedWindow, Monitor /* 以后：LiveStream */ }
pub struct SourceInfo { pub kind: SourceKind, pub label: String } // 如 "YuanShen.exe（原神）"
```
- **v1 的实现**（在 `platform` 里）：游戏窗口、指定窗口、显示器三种。
- **以后：** 直播流（新增 `live` crate，调用 ffmpeg 子进程）、macOS 和 Linux 截图。

**`QrDecoder`（二维码识别）：** 从一帧里识别出所有二维码的文本。v1 用 rxing 实现。

**`GameProfile`（游戏配置）：** 一张静态配置表，每个游戏一条记录。

```rust
pub struct GameProfile {
    pub id: &'static str,              // "hk4e_cn"
    pub display_name: &'static str,    // "原神"
    pub app_id: u32,                   // 4
    pub biz_key: &'static str,         // "hk4e_cn"
    pub scan_url: &'static str,        // https://api-sdk.mihoyo.com/hk4e_cn/combo/panda/qrcode/scan
    pub confirm_url: &'static str,     // https://api-sdk.mihoyo.com/hk4e_cn/combo/panda/qrcode/confirm
    pub process_names: &'static [&'static str], // ["YuanShen.exe"]
}
```
支持新游戏时只需要加一条配置。

**`ConfirmPolicy`（确认策略）：** 识别到合法的二维码后决定怎么做，结果是 `AskUser`（问你）、`AutoConfirm`（直接登录）或 `Ignore`（忽略）。
- v1 只有 `AlwaysAsk`：每次都问你。
- 以后的直播抢码模式加 `AutoConfirmOnce`：你手动开启后，自动确认一次就关闭。
- 硬性规则：画面来源是 `PickedWindow` 或 `Monitor` 时**永远不会**自动确认。策略如果返回 `AutoConfirm`，`ScanSession` 会强制改成 `AskUser`。这条规则写在 `core` 里，任何策略都绕不开。

**`LoginApi`（登录接口）：** 换取 game_token、scan、confirm、绑定账号相关的请求。
- 正式实现在 `api` 里。
- 测试时用假实现。
- 以后接入登录方式不同的服务器（比如崩坏3 B服）时，再加一个实现。

**`AccountStore`（账号存储）：** 读写账号文件。
- v1 的 `JsonFileStore` 放在 `core`，只用标准库读写文件，文件路径由调用方传入。
- 以后的加密存储是另一个实现。

**`ScanSession`（扫码会话状态机）：** 纯状态机，不包含线程，也不做任何 I/O。调用方把事件喂给它，比如“识别到二维码”“用户的选择”“接口返回结果”“超时”，它返回下一步该做什么，比如“弹窗询问”“调用 scan/confirm”“停止”。截图、识别和网络请求都由 `app` 或 `cli` 的运行循环负责，所以状态机可以单独测试。

## 6. 主要流程

### 6.1 绑定账号（用米游社 App 扫码）

1. 你点“绑定账号”，`api` 向米哈游申请登录二维码，Rust 端生成 SVG 交给界面显示。
2. 你用米游社 App 扫码并在手机上确认。程序每 1–2 秒查询一次状态，界面依次显示“等待扫码 / 已扫码，请在手机上确认 / 绑定成功”。
3. 确认后拿到 uid、mid、stoken，写入账号文件。
4. 二维码过期就提示刷新。v1 只支持一个账号：已有账号时再绑定，先让你确认，再替换掉旧账号。

用哪组接口在 M1 确定（第 14 节）：

- **方案 A**（来自 MHY_Scanner 公开源码）：
  1. `POST https://hk4e-sdk.mihoyo.com/hk4e_cn/combo/panda/qrcode/fetch`，参数 `{app_id, device}`，拿到二维码 URL。原项目这里用的 app_id 是 2。
  2. 轮询 `.../qrcode/query`，参数 `{app_id, device, ticket}`。`stat` 依次变为 `Init`、`Scanned`、`Confirmed`；确认后从 `payload.raw` 里取出 uid 和 game_token。
  3. `POST https://api-takumi.mihoyo.com/account/ma-cn-session/app/getTokenByGameToken`，参数 `{account_id, game_token}`，需要带米游社 App 的请求头，返回 stoken 和 mid。
- **方案 B**（出现在 MHY_Scanner v1.16.0 的 exe 里，请求格式未知）：使用 `passport-api.mihoyo.com` 的 `createQRLogin` 和 `queryQRLoginStatus`。

### 6.2 扫码登录

1. **选择截图目标：**
   - **自动**（默认）：按 `GameProfile.process_names` 找游戏窗口。找不到时显示“等待原神窗口…”，每 2 秒重找一次。
   - **指定窗口**：从可见窗口列表（标题 + 进程名）里选一个。窗口移动后截图区域跟着移动；窗口最小化时暂停，并显示“窗口已最小化”。
   - **显示器**：从显示器列表里选一个，默认主显示器。
2. **截图和识别：**
   - 每秒最多 4 帧。
   - 识别在单独的线程里进行；识别跟不上时直接丢帧，只处理最新的一帧。
   - 一帧里有多个二维码时，逐个处理。
3. **校验**：对二维码文本执行第 7 节的规则，不合格的直接忽略，不打扰你。
4. **去重**：同一个 ticket 在本次会话里只处理一次，你拒绝过的也不再弹窗。
5. **确认**：由 Rust 端通过 tauri-plugin-dialog 弹出系统原生对话框，例如：

   > 在 YuanShen.exe（原神）中发现**原神**登录二维码。
   > 用账号 1\*\*\*\*678 登录？确认后，显示这个二维码的设备将登录你的账号。
   > [登录] [取消]

   - `cli` 里没有图形界面，同样的内容改为在终端里提问，输入 y 确认。
   - 游戏名取自程序自己的 `GameProfile`，不用二维码里的 `app_name` 文字。
   - 来源描述取自 `SourceInfo`。
   - uid 只显示第一位和最后三位，中间打码。
6. **登录**：你点了“登录”，程序才依次：
   1. 换取最新的 game_token：`GET https://api-takumi.mihoyo.com/auth/api/getGameToken?stoken=…&mid=…`；
   2. 调用 scan，参数 `{app_id, device, ticket}`；
   3. 调用 confirm，参数 `{app_id, device, ticket, payload: {proto: "Account", raw: "{\"uid\":…,\"token\":<game_token>}"}}`。

   返回值 `retcode` 为 0 表示成功。
7. **结束**：
   - 登录成功后停止扫描，界面显示结果。
   - scan 或 confirm 失败时显示原因（第 10 节），二维码问题（过期、已被扫）会继续扫描，等游戏刷新二维码。
8. **超时停止**：连续一段时间没发现合法的二维码就自动停止扫描，默认 300 秒，可以在设置里改。

另外：
- **会话开始时先检查凭证。** 扫描开始前先换一次 game_token，凭证失效就立刻提示重新绑定，免得扫到码才发现用不了。
- **以后的直播抢码模式顺序相反。** 识别到二维码立刻 scan + confirm，因为 scan 要抢在别人前面。这个顺序由 `ConfirmPolicy` 和 `ScanSession` 决定，运行循环本身不用改。

## 7. 二维码校验规则

二维码文本必须同时满足下面所有条件，否则忽略：

1. 能解析成 URL，scheme 是 `https`，host **正好是** `user.mihoyo.com`（不接受子域名，也不接受相似域名），path 是 `/qr_code_in_game.html`。
2. 带有 `app_id`、`biz_key`、`ticket`、`expire` 这几个参数，而且每个参数只出现一次。
3. `app_id` 和 `biz_key` 必须同时对上 `GameProfile` 表里的同一条记录。v1 只有原神：`app_id=4`、`biz_key=hk4e_cn`。
4. `ticket` 只包含字母和数字，长度在 16 到 64 之间。目前见到的都是 24 位十六进制。
5. `expire` 是 Unix 秒，并且晚于当前时间减 60 秒。允许 60 秒误差，是为了照顾本机时钟不准。

其他参数（`app_name`、`bbs` 等）忽略，界面上也不显示。

原神客户端二维码的样例格式：
`https://user.mihoyo.com/qr_code_in_game.html?app_id=4&app_name=%E5%8E%9F%E7%A5%9E&bbs=true&biz_key=hk4e_cn&expire=1690964505&ticket=<24位十六进制>`

## 8. 数据存储（v1 明文）

数据目录是 `%LOCALAPPDATA%\MhyCarry\`。
- 不放在程序目录，免得用户打包分享程序时把账号一起发出去。
- 用 `Local` 不用 `Roaming`，免得在域环境里账号随漫游配置文件同步到服务器。

`accounts.json` 存账号凭证，**明文**：

```json
{
  "format": 1,
  "encryption": "none",
  "accounts": [
    {
      "id": "本地生成的 UUID",
      "uid": "123456789",
      "mid": "…",
      "stoken": "…",
      "note": "",
      "bound_at": "2026-10-11T08:00:00Z"
    }
  ]
}
```

`settings.json` 存设置，不含任何凭证，分享出去也没关系：

```json
{
  "format": 1,
  "device_id": "UUID v4，首次运行时生成，之后固定不变",
  "scan": { "default_target": "auto", "idle_timeout_secs": 300 }
}
```

日志写在 `logs\mhy-carry.log`，每次启动覆盖，最大 1 MB，内容保证不含 token（第 9 节）。

读写规则：
- **原子写入。** 先在同目录写临时文件，刷到磁盘后再替换原文件。
- **文件损坏时不覆盖。** 解析失败时把原文件改名为 `accounts.json.bak-<时间戳>`，提示你重新绑定。
- **遇到更新的格式时只读不写。** `format` 大于程序支持的版本，或者 `encryption` 是不认识的值，程序拒绝写入，并提示“账号文件来自更新的版本”。
- **为加密预留了字段。** 以后加密时，`format` 和 `encryption` 字段让新版程序能识别 v1 的明文文件，并自动迁移成加密格式。
- **明确告知。** README 和界面里写明 token 存在哪里、是明文、怎么删除（解绑，或者直接删掉文件）。

## 9. 安全设计

### 防护目标

| 风险 | 对策 |
|---|---|
| 凭证被发到第三方 | 网络白名单，没有遥测，不连任何第三方服务器 |
| 被诱导把账号登录到别人的设备上（二维码劫持） | 严格校验二维码（第 7 节）；每次都要你确认；弹窗写明二维码来自哪里；自动确认永远不对“指定窗口”和“显示器”开放 |
| 发布的 exe 被替换，或者和源码对不上 | 只由 CI 构建，附构建来源证明和 SHA256，开启不可变发布 |
| 依赖被投毒 | 依赖尽量少，`Cargo.lock` 锁定版本，cargo-deny 检查，GitHub Actions 引用固定到 commit SHA |

**v1 防不了的：**
- **账号文件泄露。** 文件被人拷走、被同步进网盘，或者电脑中了木马，token 都会泄露，因为是明文。这是 v1 的已知限制，README 会写明。
- **扫整个屏幕时程序能看到屏幕上的所有内容。** 画面只在内存里识别，不存盘、不上传，程序只使用识别出的二维码文本。

### 网络
- HTTP 客户端只能访问白名单域名，发请求前先检查 host。只允许 HTTPS，禁止重定向（`redirect::Policy::none()`），超时 10 秒。
- 白名单最终只保留 M1 之后实际用到的域名，候选有：`api-takumi.mihoyo.com`、`api-sdk.mihoyo.com`、`hk4e-sdk.mihoyo.com`（方案 A）、`passport-api.mihoyo.com`（方案 B）。
- 证书校验用 rustls 和 webpki 根证书的默认行为，不提供“忽略证书错误”之类的选项。
- 测试时需要连本地假服务器，所以可以替换白名单和放行 http 的构造函数只放在 `test-support` feature 后面，正式构建不启用这个 feature。

### 界面隔离（Tauri）
- **token 只存在于 Rust 一侧。** 界面能拿到的只有打码后的 uid、备注、运行状态和截图目标列表。
- **确认对话框由 Rust 端弹出。** 网页端没有 dialog 权限，网页里的代码没法替你点确认。
- **能力收到最小：**
  - capabilities 只开放我们自己的命令（通过 app manifest 生成权限）和事件监听，不启用文件、Shell、HTTP、opener 等插件；
  - CSP 只允许本地资源，`img-src` 额外允许 `data:`，用来显示绑定二维码的 SVG；
  - 二维码文本、窗口标题这类外部文本一律用 `textContent` 显示，不用 `innerHTML`。

界面和 Rust 之间的接口（v1）：
- **命令：**
  - `get_state`；
  - `bind_start` / `bind_cancel` / `unbind`；
  - `list_targets`；
  - `scan_start {target}` / `scan_stop`；
  - `update_settings`。
- **事件：** `bind-status`、`scan-status`。

### 日志
- stoken、game_token 用 `secrecy::SecretString` 包装，打印时显示成 `[REDACTED]`。
- `getGameToken` 的 stoken 在 URL 参数里，而 reqwest 的错误信息会带上完整 URL。所以记日志前先用 `reqwest::Error::without_url()` 去掉 URL。
- 有一个测试专门检查：错误信息和日志输出里不会出现 token 原文。

## 10. 错误处理

- **服务器返回什么都不崩溃。** 返回内容一律按定义好的结构解析，字段缺失或格式不对只当作普通错误处理。后台任务里的错误都通过事件传回界面，不会让程序退出。
- **错误分类：**

| 类别 | 例子 | 界面提示 |
|---|---|---|
| 凭证失效 | stoken 过期、getGameToken 失败 | 请重新绑定账号 |
| 二维码问题 | 已过期、已被扫、ticket 无效 | 请在游戏里刷新二维码（继续扫描） |
| 环境问题 | 找不到窗口、窗口最小化、截图失败、断网 | 具体原因和建议 |
| 接口变化 | 未知 retcode、返回格式不对 | 显示 retcode 和服务器返回的 message，提示接口可能已变更，请提 Issue |

- **重试规则：**
  - 只有重复发送也没有副作用的请求（查询绑定状态、getGameToken）才自动重试，最多一次。
  - scan 和 confirm 失败后不自动重试。
- **截图失败：** 偶尔一帧失败直接跳过；连续 10 帧失败就停止扫描并提示原因。
- **账号文件读写失败：** 处理规则见第 8 节。

## 11. 测试

- **`core`（单元测试，在 Windows、macOS、Linux 的 CI 上都跑）：**
  - **二维码 URL 校验：** 合法样例，加上每条规则的反例：
    - `http://` 链接；
    - 相似域名：`user.mihoyo.com.evil.cn`、`evil.cn/user.mihoyo.com`、带 `@` 的 userinfo；
    - 子域名、错误的 path、缺参数、重复参数；
    - `app_id` 和 `biz_key` 对不上、未知的 `biz_key`；
    - 已过期、ticket 带非法字符或长度不对。

    再用 proptest 大量随机输入，保证解析器不崩溃，也不会把不合法的输入判为合法。
  - **二维码识别：** 测试里用 qrcode 生成二维码，缩放后贴到带噪点的背景上，检查能识别出来。再加几张自己截的真实游戏截图。截图里的 ticket 已经过期，没有泄露风险。
  - **`ScanSession`：** 用假的画面来源和假的 `LoginApi`，测这些情况：
    - 窗口模式下先确认后 scan；
    - ticket 去重；
    - 拒绝后不再弹窗；
    - 超时自动停止；
    - scan 失败后继续扫描；
    - confirm 失败后不重试；
    - `PickedWindow` 和 `Monitor` 不能自动确认。
  - **`JsonFileStore`：** 原子写入、损坏文件改名备份、遇到更新的格式时拒绝写入。
- **`api`（用 wiremock 在本地起假服务器测）：**
  - 正常响应、各种 retcode 和格式错误的响应；
  - 会被拒绝的情况：重定向、白名单外的 host、非 HTTPS（`test-support` 构造函数之外）；
  - token 不会出现在错误信息和日志里。
- **`platform` 和应用界面（v1 在 Windows 上按清单手动测）：**
  - 原神以管理员权限运行时能截图；
  - 多显示器；
  - 150% 缩放；
  - 浏览器窗口；
  - 窗口最小化后恢复；
  - 中途关掉游戏窗口。
- **真实服务器：** 只用自己的账号、通过 `cli` 手动测。CI 里不放任何真实凭证。

## 12. 构建、CI 与发布

- **本地开发：**
  - `cargo test --workspace`；
  - `cargo run -p mhy-carry-cli -- <子命令>`；
  - `cargo tauri dev`（在 `app/` 目录下运行）。
- **`ci.yml`**（每次 push 和 PR 都跑）：
  - `cargo fmt --check`；
  - `cargo clippy --workspace --all-targets -- -D warnings`：只在 Windows 上跑，因为 `platform` 只有 Windows 实现，在 Linux 上检查 Tauri 应用还要额外装 WebKitGTK；
  - `cargo test`：`core` 和 `api` 在 Windows、macOS、Linux 上跑，其余只在 Windows 上跑；
  - `cargo deny check`：检查许可证、RustSec 漏洞库和依赖来源。它已经包含 `cargo audit` 的漏洞检查，所以不再单独跑 `cargo audit`。
- **`release.yml`**（推送 `v*` tag 时触发，只在 Windows 上构建）：
  1. 构建 NSIS 安装包和免安装 exe；
  2. 生成 `SHA256SUMS`；
  3. 用 `actions/attest-build-provenance` 生成构建来源证明，用户可以用 `gh attestation verify <文件> -R shulkx/mhy-carry` 核对；
  4. 创建 GitHub Release。仓库开启不可变发布（immutable releases），发布后的文件不能替换。
- **其他：**
  - 所有第三方 Action 都固定到 commit SHA。
  - 代码签名以后再做，可以申请 SignPath 的免费开源签名。在那之前，README 说明 SmartScreen 警告是正常的，并给出核对构建来源证明的方法。

## 13. 里程碑与验收标准

| 里程碑 | 内容 | 验收标准 |
|---|---|---|
| M0 仓库骨架 | workspace、各 crate 空壳、`rust-toolchain.toml`、`deny.toml`、`ci.yml`、MIT 许可证、README（含声明和安全说明） | CI 全部通过 |
| M1 协议和截图验证 | `cli bind`（方案 A，不行再做方案 B）、`cli login <二维码URL>`、截图方案对比实验 | 用自己的账号，通过 `cli` 把原神客户端登录成功；第 14 节的三项实测都有结论，并更新到本文档 |
| M2 自动扫描 | `cli watch --target auto\|window\|monitor`：`ScanSession`、三种截图目标、在终端里确认 | 三种目标各成功登录一次；第 11 节的 `core` 和 `api` 测试全部通过 |
| M3 Tauri 应用 | 界面、Rust 端对话框、capabilities 和 CSP 收紧 | 第 11 节的手动检查清单全部通过；网页端调用不到 dialog、文件等能力 |
| M4 发布 v0.1.0 | `release.yml`、使用文档 | Release 带构建来源证明和 SHA256，`gh attestation verify` 核对通过 |

## 14. 需要在 M1 实测确定的事项

1. **绑定接口用哪组。** 先实现方案 A，能拿到 uid、mid、stoken 就用 A。A 不行再研究方案 B 的请求格式，参考其他开源项目和公开资料。最终用的方案和 B 的请求格式都更新到第 6.1 节。
2. **截图用 xcap 还是 windows-capture（Windows.Graphics.Capture）。** 合格标准：
   - 本程序以普通权限运行，能截到以管理员权限运行的原神窗口的真实画面（不是黑屏）；
   - 不需要管理员权限，也不弹系统授权提示；
   - 在 4K 屏上维持每秒 4 帧，本程序 CPU 占用不超过一个核心的 25%。

   两个都合格就选依赖更少的那个。Windows.Graphics.Capture 截图时窗口会出现黄色边框，这个可以接受，在 README 里说明即可。
3. **浏览器里云原神的二维码。** 记录二维码的实际 URL：参数和客户端一样，就不用改；不一样，就在 `GameProfile` 表里加一条（比如云原神对应的 `app_id`、`biz_key`）。

## 15. 许可证与声明

- **许可证：** MIT。
- **独立实现。** MHY_Scanner 是 GPL-3.0，MhyCarry **不复制它的任何代码**，只参考接口地址和协议格式这类公开信息。
- **README 写明：**
  - 本项目是非官方工具，与米哈游无关；
  - 使用了非公开接口，接口可能随时变化，使用者自行承担账号风险；
  - v1 只支持国服官服；
  - 账号凭证以明文存储，以及存储位置和删除方法。
