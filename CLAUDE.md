# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## 项目

MhyCarry：识别屏幕上的米哈游游戏登录二维码，用用户绑定的账号确认登录（常见用法：请主播或朋友帮打难关）。Rust workspace + 以后的 Tauri 2 应用，v1 只做 Windows、原神国服官服、一个账号。用户用中文交流。

- 设计：[docs/design/2026-10-11-mhy-carry-design.md](docs/design/2026-10-11-mhy-carry-design.md)，已经和用户逐节确认过。改动设计前先问用户。
- 进度：M0、M1、M2 已完成（计划见 [docs/plans/](docs/plans/)，实测结论在设计文档第 14 节）。M3（Tauri 应用）代码已完成，还差用户用真实账号实测。M4（发布 v0.1.0）按 [docs/plans/2026-10-14-m4-plan.md](docs/plans/2026-10-14-m4-plan.md) 进行中。
- 数据目录是 `%LOCALAPPDATA%\io.github.shulkx.mhycarry\`（Tauri 应用标识，设计文档第 8 节）；安装包只为当前用户安装，程序装在 `%LOCALAPPDATA%\MhyCarry\`。

## 执行计划

- 在当前会话里按顺序执行（executing-plans），不要每个任务开一个子代理。在这台机器上子代理很慢：一次 118 轮的调研跑了 24 分钟，原因是轮数多、上下文大，而且访问 GitHub 慢。
- 计划里每一步都有完整代码和预期输出。实际结果对不上时，先查清原因再改，不要为了让测试通过而改测试。
- 一个里程碑的代码写完后，派一个子代理对这批改动做一次只读的独立审查，修完再让用户实测。M1、M2 的审查都发现过真问题。
- 需要用户本人操作游戏或米游社 App 的步骤，提前告诉用户要做什么。
- 提交时带上 `Cargo.lock`。

## 常用命令

```powershell
cargo test --workspace --all-features            # 全部测试
cargo test -p mhy-carry-core scan::              # 某个模块的测试
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo fmt --all --check
cargo deny check                                 # 许可证、漏洞库、依赖来源
$env:MHY_CARRY_DATA_DIR = "$env:TEMP\mhy-carry-dev"   # 调试时避开真实账号文件
cargo run -p mhy-carry-cli -- account | bind | login <URL> | login --image <截图>
cargo run --release -p mhy-carry-cli -- watch [--target auto|window|monitor]
cargo run -p mhy-carry                           # 图形界面（调试版，右键“检查”可打开开发者工具）
Push-Location app; cargo tauri build --bundles nsis --ci; Pop-Location   # 安装包，需要 tauri-cli 2.12.0
```

每次提交前，fmt、clippy、测试都要通过。

## 约定

- 文档、代码注释、界面和命令行文字、提交信息都用中文；标识符用英文。提交信息格式：`<范围>: <做了什么>`，如 `core: 添加二维码校验`。
- 文档放在常规位置：设计放 `docs/design/`，计划放 `docs/plans/`。不要用 `docs/superpowers/` 这类路径。
- 文本文件用 LF（由 `.gitattributes` 统一）。不要照搬 MHY_Scanner 的 BOM、CRLF 约定。
- 本项目是 MIT 许可证，独立实现：不复制 MHY_Scanner（GPL-3.0）或其他项目的代码，只参考接口地址、字段这类协议信息。
- 窗口标题、服务器返回的文字这类外部文本，显示前都用 `mhy_carry_core::text::printable` 清理。

## 米哈游接口

- 新增或修改任何米哈游接口前，先对照**近期仍在维护**的开源项目核对：看仓库的 `pushed_at` 和相关文件的最后提交时间。接口变化很快，2026 年就废弃过一整套流程。核对来源记到设计文档第 16 节。
- 设计文档第 6 节是当前的接口约定，第 14 节是用真实账号实测的结论。接口有变化时，实测结论要写回这两节。
- 网络白名单只有 `api-sdk.mihoyo.com` 和 `passport-api.mihoyo.com`（设计文档第 9 节）。新增域名要同时改 `crates/api/src/guard.rs` 和设计文档，并告诉用户。不加遥测，也不连任何第三方服务器。

## 安全红线

- stoken、ticket、Cookie、`accounts.json` 的内容，不贴进聊天、不提交、不写进文档或日志。实测时只记录字段名和错误码。含二维码的截图也不提交。
- 用真实账号测试只能由用户本人手动操作；CI 里不放任何凭证。
- 对外的操作先问用户：创建或推送 GitHub 仓库、发布 Release、改仓库设置。

## 本机环境

- Windows，项目在 `E:\GitHub\mhy-carry`。工具链在 2026-10-11 装好：Rust 1.99.0（rustup，带 rustfmt、clippy）、VS 2022 生成工具 17.14（MSVC 14.44）、cargo-deny 0.20.2。
- 显示器是 4K、150% 缩放；原神以管理员权限运行，本程序以普通权限运行。
- 公司网络疑似会解密检查 HTTPS：单次 GitHub API 请求约 1.4 秒；curl 访问外网要加 `--ssl-no-revoke`，否则会报证书吊销检查失败。
- `gh search code` 每分钟限 10 次。独立的查询放在同一轮里一起发，不要串行一个个查。
