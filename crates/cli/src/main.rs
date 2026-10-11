mod account_cmd;
mod bind_cmd;
mod context;
mod login_cmd;
mod prompt;

use std::path::PathBuf;
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
    /// 用米游社 App 扫码绑定账号
    Bind,
    /// 查看已绑定的账号，或者解绑
    Account {
        /// 解绑，删除本机保存的账号凭证
        #[arg(long)]
        remove: bool,
    },
    /// 用已绑定的账号确认一个游戏登录二维码
    Login {
        /// 二维码里的网址
        #[arg(required_unless_present = "image", conflicts_with = "image")]
        url: Option<String>,
        /// 含二维码的截图文件（PNG 或 JPG）
        #[arg(long)]
        image: Option<PathBuf>,
    },
}

pub type CliResult = Result<(), Box<dyn std::error::Error>>;

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = match cli.command {
        Command::Bind => bind_cmd::run().await,
        Command::Account { remove } => account_cmd::run(remove),
        Command::Login { url, image } => {
            let source = match (url, image) {
                (Some(url), _) => login_cmd::QrSource::Url(url),
                (None, Some(path)) => login_cmd::QrSource::Image(path),
                (None, None) => unreachable!("clap 保证网址和截图至少有一个"),
            };
            login_cmd::run(source).await
        }
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("错误：{error}");
            ExitCode::FAILURE
        }
    }
}
