# MhyCarry

扫描屏幕上的米哈游游戏登录二维码，用你绑定的账号确认登录。常见用法：请朋友或主播帮你打难关时，扫对方游戏里显示的登录二维码，把账号登录到对方那里。

> 开发中。目前只有命令行工具 `mhy-carry-cli`，图形界面还没做。

## 声明

- 本项目是非官方工具，与米哈游（miHoYo）无关。
- 使用了米哈游的非公开接口，接口可能随时变化；使用本工具的账号风险由使用者自行承担。
- 目前只支持**原神国服官服**。
- 账号凭证（stoken）以**明文**保存在 `%LOCALAPPDATA%\MhyCarry\accounts.json`。不要把这个文件发给任何人。运行 `mhy-carry-cli account --remove` 解绑即可清除凭证，它会一并删除账号文件损坏时留下的 `accounts.json.bak-*` 备份；也可以直接删除整个 `%LOCALAPPDATA%\MhyCarry` 文件夹。
- 程序只连接米哈游官方域名，没有遥测，不连任何第三方服务器。

## 从源码构建

需要 Rust（rustup 会按 `rust-toolchain.toml` 自动安装对应版本）和 Visual Studio C++ 生成工具。

    cargo build --release -p mhy-carry-cli

## 许可证

MIT
