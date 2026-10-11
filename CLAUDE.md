# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## 项目

MhyCarry：识别屏幕上的米哈游游戏登录二维码，用用户绑定的账号确认登录（常见用法：请主播或朋友帮打难关）。Rust workspace + 以后的 Tauri 2 应用，v1 只做 Windows、原神国服官服、一个账号。用户用中文交流。

- 设计：[docs/design/2026-10-11-mhy-carry-design.md](docs/design/2026-10-11-mhy-carry-design.md)，已经和用户逐节确认过。改动设计前先问用户。
- 当前计划：[docs/plans/2026-10-11-m0-m1-plan.md](docs/plans/2026-10-11-m0-m1-plan.md)（M0 仓库骨架 + M1 命令行验证）。M2–M4 的计划等 M1 的实测结论写进设计文档后再写。

## 执行计划

- 在当前会话里按顺序执行（executing-plans），不要每个任务开一个子代理。在这台机器上子代理很慢：一次 118 轮的调研跑了 24 分钟，原因是轮数多、上下文大，而且访问 GitHub 慢。
- 在这几个地方停下来，给用户看结果：任务 3（M0 完成）、任务 11（`core` 完成）、任务 15（`api` 完成）、任务 18（`cli` 完成）。
- 任务 19（DS2 签名）只在任务 21 实测需要时才做。
- 任务 20、21 需要用户本人操作游戏和米游社 App，提前告诉用户要做什么。
- 计划里每一步都有完整代码和预期输出。实际结果对不上时，先查清原因再改，不要为了让测试通过而改测试。

## 常用命令

```powershell
cargo test --workspace --all-features            # 全部测试
cargo test -p mhy-carry-core qr                  # 某个 crate 里名字含 qr 的测试
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo fmt --all --check
cargo deny check                                 # 许可证、漏洞库、依赖来源
$env:MHY_CARRY_DATA_DIR = "$env:TEMP\mhy-carry-dev"   # 调试时避开真实账号文件
cargo run -p mhy-carry-cli -- account | bind | login <URL> | login --image <截图>
```

每次提交前，fmt、clippy、测试都要通过。

## 约定

- 文档、代码注释、界面和命令行文字、提交信息都用中文；标识符用英文。提交信息格式：`<范围>: <做了什么>`，如 `core: 添加二维码校验`。
- 文档放在常规位置：设计放 `docs/design/`，计划放 `docs/plans/`。不要用 `docs/superpowers/` 这类路径。
- 文本文件用 LF（由 `.gitattributes` 统一）。不要照搬 MHY_Scanner 的 BOM、CRLF 约定。
- 本项目是 MIT 许可证，独立实现：不复制 MHY_Scanner（GPL-3.0）或其他项目的代码，只参考接口地址、字段这类协议信息。

## 米哈游接口

- 新增或修改任何米哈游接口前，先对照**近期仍在维护**的开源项目核对：看仓库的 `pushed_at` 和相关文件的最后提交时间。接口变化很快，2026 年就废弃过一整套流程。核对来源记到设计文档第 16 节。
- 设计文档第 6 节是当前的接口约定，第 14 节是还要用真实账号确认的事项。实测结论要写回这两节。
- 网络白名单只有 `api-sdk.mihoyo.com` 和 `passport-api.mihoyo.com`（设计文档第 9 节）。新增域名要同时改 `crates/api/src/guard.rs` 和设计文档，并告诉用户。不加遥测，也不连任何第三方服务器。

## 安全红线

- stoken、ticket、Cookie、`accounts.json` 的内容，不贴进聊天、不提交、不写进文档或日志。实测时只记录字段名和错误码。
- 用真实账号测试只能由用户本人通过 `cli` 手动操作；CI 里不放任何凭证。
- 对外的操作先问用户：创建或推送 GitHub 仓库、发布 Release、改仓库设置。

## 本机环境

- Windows，项目在 `E:\GitHub\mhy-carry`。工具链已在 2026-10-11 装好（计划任务 1 已完成）：Rust 1.99.0（rustup，带 rustfmt、clippy）、VS 2022 生成工具 17.14（MSVC 14.44）、cargo-deny 0.20.2。reqwest 0.13 的 aws-lc-rs 已在本机编译通过，TLS 访问 `passport-api.mihoyo.com` 正常。
- 公司网络疑似会解密检查 HTTPS：单次 GitHub API 请求约 1.4 秒；curl 访问外网要加 `--ssl-no-revoke`，否则会报证书吊销检查失败。
- `gh search code` 每分钟限 10 次。独立的查询放在同一轮里一起发，不要串行一个个查。
