# MhyCarry

扫描屏幕上的米哈游游戏登录二维码，用你绑定的账号确认登录，不用掏出手机。常见用法：请朋友或主播帮你打难关时，扫对方游戏里显示的登录二维码，把账号登录到对方那里。

也可以直接拉 **B 站直播间**的直播流，主播一亮出登录码就抢着登录。

目前只支持 Windows 和**原神国服官服**。可以绑定多个账号，扫码时用选好的那一个。

## 下载和安装

在 [Releases](https://github.com/shulkx/mhy-carry/releases) 页面下载：

- `MhyCarry_<版本>_x64-setup.exe`：安装包。只为当前用户安装，不需要管理员权限，程序装在 `%LOCALAPPDATA%\MhyCarry\`。
- `MhyCarry_<版本>_x64-portable.exe`：免安装版，下载后直接运行。

两种都需要 WebView2 运行时：Windows 11 自带，Windows 10 一般也已经装好；缺少时，安装包会自动下载安装。

程序没有代码签名，第一次运行时 Windows SmartScreen 可能提示“未知发布者”，这是正常的。可以先按下一节核对文件，再点“更多信息 → 仍要运行”。

## 核对下载的文件

Release 里的文件都由 GitHub Actions 从本仓库的源码构建，并附有构建来源证明。装有 [GitHub CLI](https://cli.github.com/) 的话，运行：

    gh attestation verify MhyCarry_<版本>_x64-setup.exe -R shulkx/mhy-carry

显示 `Verification succeeded` 就说明文件确实由本仓库的 CI 构建、没有被替换过。

也可以在 PowerShell 里算哈希值，对照 Release 里的 `SHA256SUMS`：

    Get-FileHash MhyCarry_<版本>_x64-setup.exe

## 使用

1. **绑定账号。** 点“用米游社扫码绑定”，用米游社 App 扫描窗口里的二维码，然后在手机上确认。绑定后会自动查这个账号的原神角色，卡片上显示原神昵称、等级、原神 UID 和米哈游通行证 ID。
2. **选择截图目标，开始扫描。**
   - **自动**：自动找原神窗口，原神在最前面时才扫描。
   - **指定窗口**：从列表里选一个窗口，比如正在看直播的浏览器。
   - **显示器**：扫描整个屏幕。
   - **直播间**：填 B 站直播间网址或房间号，直接拉直播流识别，见下面的“直播间抢码”。
3. **确认登录。** 发现登录二维码时，会弹出一个置顶的确认框，写明二维码在哪里发现、要用哪个账号登录（账号名称、原神 UID 和通行证 ID）。核对无误再点“确定”；默认按钮是“取消”，误按回车不会登录。原神全屏时如果看不到确认框，按 Alt+Tab 切出游戏。

登录成功后自动停止扫描；连续 5 分钟没发现新的登录二维码也会停止，时间可以在“设置”里改（直播间除外）。

**只确认你信任的人给你的二维码。** 确认登录，就是把你的账号登录到显示这个二维码的设备上。

### 直播间抢码

- 选“直播间”，填直播间网址或房间号，比如 `https://live.bilibili.com/12345` 或 `12345`。下次会自动填上。暂不支持 `b23.tv` 短链接，请在浏览器里打开后复制地址栏里的网址。
- 程序直接拉直播流解码识别，不需要打开浏览器，也不受窗口遮挡影响；比在浏览器里看直播再截图快。
- 主播还没开播时显示“等待开播”，开播后自动连接；断线会自动重连。直播间不会因为一直没出码而停止，只能手动停止。
- 要抢得快，就打开“开始扫描”右边的 **自动确认**：识别到原神登录码立刻登录，不再弹确认框。开着时程序每 45 秒向米哈游的两个域名发一个不带凭证的请求，保持连接。登录成功后会显示从发现二维码到登录完成用了几秒。

**自动确认有风险。** 开着时，画面里出现的任何原神登录码都会直接用你的账号登录，包括别人故意放出的码。只在你信任的直播间里开，抢完就关。自动确认在四种截图目标下都能开，记在设置里，扫描中不能改。

### 多个账号

- 点“+ 添加账号”绑定更多账号。扫码时在米游社 App 里登录的是哪个账号，就添加哪个；已经绑定过的账号只更新凭证。刚绑定的账号自动设为“使用中”。
- 扫描用的是卡片上显示的“使用中”的账号，大按钮上也写着它的名字，比如“开始扫描（大号）”。用“切换账号”换，扫描中不能换。
- 卡片右上角的“⋯”里可以编辑备注、重新绑定、解绑，都作用于使用中的账号。要编辑或解绑别的账号，先切换过去。
- 账号名称按“备注 → 原神昵称 → 通行证 ID”取第一个有值的。备注只存在本机。

## 账号凭证存在哪里

- 账号凭证（stoken）以**明文**保存在 `%LOCALAPPDATA%\io.github.shulkx.mhycarry\accounts.json`。不要把这个文件发给任何人。
- 删除凭证的方法：
  - 在程序里解绑一个账号，只删这个账号的凭证；
  - 账号文件损坏时留下的 `accounts.json.bak-*` 备份里也可能有凭证，有的话界面上会出现“删除旧账号文件”按钮；
  - 卸载时勾选“删除应用程序数据”，会删掉整个 `io.github.shulkx.mhycarry` 文件夹；
  - 直接删掉这个文件夹。
- 删除本机凭证不会让它在服务器端失效。担心凭证泄露时，在米游社 App 里下线设备或修改密码。

## 声明

- 本项目是非官方工具，与米哈游（miHoYo）无关。
- 使用了米哈游的非公开接口，接口可能随时变化；使用本工具的账号风险由使用者自行承担。
- 账号凭证只发给米哈游官方域名（`api-sdk.mihoyo.com`、`passport-api.mihoyo.com`）。
- 用直播间抢码时，程序会连接 B 站直播接口（`api.live.bilibili.com`）和它返回的直播 CDN，不带任何凭证或 Cookie。
- 没有遥测，不连其他服务器。
- 截到的画面和直播画面只在内存里识别二维码，不保存，也不上传。
- 直播间抢码用的是 Windows 自带的 H.264 解码器。Windows N 版要先装“媒体功能包”。

## 从源码构建

需要 Rust（rustup 会按 `rust-toolchain.toml` 自动安装对应版本）和 Visual Studio C++ 生成工具。

    cargo build --release -p mhy-carry              # 图形界面，生成 target\release\mhy-carry.exe
    cargo install tauri-cli --locked --version 2.12.0
    cd app; cargo tauri build --bundles nsis        # 安装包

还有一个命令行工具，主要用来调试：

    cargo run --release -p mhy-carry-cli -- account                         # 列出账号，* 是使用中的
    cargo run --release -p mhy-carry-cli -- account --refresh               # 检查凭证、查原神角色
    cargo run --release -p mhy-carry-cli -- bind                            # 扫码添加账号
    cargo run --release -p mhy-carry-cli -- account --use 300012345         # 切换使用中的账号
    cargo run --release -p mhy-carry-cli -- account --note 300012345 大号   # 写备注
    cargo run --release -p mhy-carry-cli -- account --remove 300012345      # 解绑
    cargo run --release -p mhy-carry-cli -- watch --target auto             # 扫描，auto | window | monitor | live
    cargo run --release -p mhy-carry-cli -- watch --target live --room 12345 --auto-confirm   # 直播间抢码，自动确认

## 许可证

MIT
