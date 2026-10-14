# MhyCarry

扫描屏幕上的米哈游游戏登录二维码，用你绑定的账号确认登录，不用掏出手机。常见用法：请朋友或主播帮你打难关时，扫对方游戏里显示的登录二维码，把账号登录到对方那里。

目前只支持 Windows 和**原神国服官服**，只能绑定一个账号。

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

1. **绑定账号。** 点“用米游社扫码绑定”，用米游社 App 扫描窗口里的二维码，然后在手机上确认。
2. **选择截图目标，开始扫描。**
   - **自动**：自动找原神窗口，原神在最前面时才扫描。
   - **指定窗口**：从列表里选一个窗口，比如正在看直播的浏览器。
   - **显示器**：扫描整个屏幕。
3. **确认登录。** 发现登录二维码时，会弹出一个置顶的确认框，写明二维码在哪里发现、要用哪个账号登录。核对无误再点“确定”；默认按钮是“取消”，误按回车不会登录。原神全屏时如果看不到确认框，按 Alt+Tab，或者点任务栏上闪烁的 MhyCarry。

登录成功后自动停止扫描；连续 5 分钟没发现新的登录二维码也会停止，时间可以在“设置”里改。

**只确认你信任的人给你的二维码。** 确认登录，就是把你的账号登录到显示这个二维码的设备上。

## 账号凭证存在哪里

- 账号凭证（stoken）以**明文**保存在 `%LOCALAPPDATA%\io.github.shulkx.mhycarry\accounts.json`。不要把这个文件发给任何人。
- 删除凭证的方法：
  - 在程序里点“解绑”，会一并删除账号文件损坏时留下的 `accounts.json.bak-*` 备份；
  - 卸载时勾选“删除应用程序数据”，会删掉整个 `io.github.shulkx.mhycarry` 文件夹；
  - 直接删掉这个文件夹。
- 删除本机凭证不会让它在服务器端失效。担心凭证泄露时，在米游社 App 里下线设备或修改密码。

## 声明

- 本项目是非官方工具，与米哈游（miHoYo）无关。
- 使用了米哈游的非公开接口，接口可能随时变化；使用本工具的账号风险由使用者自行承担。
- 程序只连接米哈游官方域名（`api-sdk.mihoyo.com`、`passport-api.mihoyo.com`），没有遥测，不连任何第三方服务器。
- 截到的画面只在内存里识别二维码，不保存，也不上传。

## 从源码构建

需要 Rust（rustup 会按 `rust-toolchain.toml` 自动安装对应版本）和 Visual Studio C++ 生成工具。

    cargo build --release -p mhy-carry              # 图形界面，生成 target\release\mhy-carry.exe
    cargo install tauri-cli --locked --version 2.12.0
    cd app; cargo tauri build --bundles nsis        # 安装包

还有一个命令行工具，主要用来调试：

    cargo run --release -p mhy-carry-cli -- account                      # 查看绑定的账号
    cargo run --release -p mhy-carry-cli -- bind                         # 扫码绑定
    cargo run --release -p mhy-carry-cli -- watch --target auto          # 扫描，auto | window | monitor
    cargo run --release -p mhy-carry-cli -- account --remove             # 解绑

## 许可证

MIT
