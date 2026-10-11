use clap::Parser;

/// MhyCarry 命令行工具：验证协议和调试用。
#[derive(Parser)]
#[command(name = "mhy-carry-cli", version)]
struct Cli {}

fn main() {
    let _cli = Cli::parse();
}
