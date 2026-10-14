mod account_cmd;
mod bind_cmd;
mod context;
mod login_cmd;
mod prompt;
mod watch_cmd;

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand};

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
    /// 查看已绑定的账号；切换使用中的账号、写备注、解绑
    Account(AccountArgs),
    /// 用已绑定的账号确认一个游戏登录二维码
    Login {
        /// 二维码里的网址
        #[arg(required_unless_present = "image", conflicts_with = "image")]
        url: Option<String>,
        /// 含二维码的截图文件（PNG 或 JPG）
        #[arg(long)]
        image: Option<PathBuf>,
    },
    /// 自动截图识别登录二维码，发现后问你要不要登录
    Watch {
        /// 截图目标；不填时用设置里的默认值（auto）
        #[arg(long, value_enum)]
        target: Option<watch_cmd::Target>,
        /// 用哪个账号（通行证 ID）；不填时用使用中的账号
        #[arg(long, value_name = "通行证ID")]
        account: Option<String>,
    },
}

/// 不带参数时列出账号。下面几个参数一次只能用一个。
#[derive(Args)]
#[group(multiple = false)]
struct AccountArgs {
    /// 逐个检查凭证、查原神角色（要联网）
    #[arg(long)]
    refresh: bool,
    /// 设为使用中的账号
    #[arg(long = "use", value_name = "通行证ID")]
    use_account: Option<String>,
    /// 给账号写备注；备注写 "" 表示去掉
    #[arg(long, num_args = 2, value_names = ["通行证ID", "备注"], allow_hyphen_values = true)]
    note: Option<Vec<String>>,
    /// 解绑这个账号，删除本机保存的凭证
    #[arg(long, value_name = "通行证ID")]
    remove: Option<String>,
    /// 删除账号文件损坏时留下的旧文件
    #[arg(long)]
    remove_leftovers: bool,
}

impl AccountArgs {
    fn action(self) -> account_cmd::Action {
        use account_cmd::Action;
        if self.refresh {
            return Action::Refresh;
        }
        if self.remove_leftovers {
            return Action::RemoveLeftovers;
        }
        if let Some(uid) = self.use_account {
            return Action::Use(uid);
        }
        if let Some(uid) = self.remove {
            return Action::Remove(uid);
        }
        if let Some([uid, note]) = self
            .note
            .and_then(|pair| <[String; 2]>::try_from(pair).ok())
        {
            return Action::Note { uid, note };
        }
        Action::List
    }
}

pub type CliResult = Result<(), Box<dyn std::error::Error>>;

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = match cli.command {
        Command::Bind => bind_cmd::run().await,
        Command::Account(args) => account_cmd::run(args.action()).await,
        Command::Login { url, image } => {
            let source = match (url, image) {
                (Some(url), _) => login_cmd::QrSource::Url(url),
                (None, Some(path)) => login_cmd::QrSource::Image(path),
                (None, None) => unreachable!("clap 保证网址和截图至少有一个"),
            };
            login_cmd::run(source).await
        }
        Command::Watch { target, account } => watch_cmd::run(target.map(Into::into), account).await,
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("错误：{error}");
            ExitCode::FAILURE
        }
    }
}
