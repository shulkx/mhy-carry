mod account_cmd;
mod context;
mod prompt;

use std::process::ExitCode;

use clap::{Parser, Subcommand};

/// MhyCarry 命令行工具：验证协议和调试用。
#[derive(Parser)]
#[command(name = "mhy-carry-cli", version)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// 查看已绑定的账号，或者解绑
    Account {
        /// 解绑，删除本机保存的账号凭证
        #[arg(long)]
        remove: bool,
    },
}

pub type CliResult = Result<(), Box<dyn std::error::Error>>;

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = match cli.command {
        Command::Account { remove } => account_cmd::run(remove),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("错误：{error}");
            ExitCode::FAILURE
        }
    }
}
