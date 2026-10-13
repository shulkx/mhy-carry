//! 扫描：用共用的运行循环；登录确认用置顶的系统确认框（设计文档第 6.2 节第 5 步）。

use mhy_carry_core::scan::{Prompt, ScanStatus};
use mhy_carry_platform::dialog;
use mhy_carry_runner::{ScanJob, ScanUi, run_scan};
use tauri::{AppHandle, Manager};
use tokio::sync::mpsc;

use crate::model::{Shared, update};
use crate::state::{AccountView, credential_after};

/// 在后台跑一次扫描，结束后清掉“停止”信号。
pub fn spawn(app: AppHandle, job: ScanJob, stop: mpsc::Receiver<()>) {
    tauri::async_runtime::spawn(async move {
        let ui = WindowUi { app: app.clone() };
        let shared = app.state::<Shared>();
        run_scan(&shared.api, job, &ui, stop).await;
        update(&app, |model| model.scan_stop = None);
    });
}

/// 图形界面：状态推送给网页端，提问用系统确认框。
struct WindowUi {
    app: AppHandle,
}

impl ScanUi for WindowUi {
    fn show(&self, status: &ScanStatus) {
        update(&self.app, |model| {
            if let (
                Some(credential),
                AccountView::Bound {
                    credential: current,
                    ..
                },
            ) = (credential_after(status), &mut model.account)
            {
                *current = credential;
            }
            model.scan = status.clone();
        });
    }

    async fn ask(&self, prompt: Prompt) -> bool {
        confirm(&self.app, prompt.question(), true).await
    }
}

/// 在 Rust 端弹出系统确认框（网页端没有对话框权限，设计文档第 9 节）。
///
/// 登录确认（`topmost`）不挂在主窗口下面：主窗口可能最小化了，挂在它下面的确认框会
/// 跟着隐藏；同时闪烁主窗口的任务栏按钮。解绑、替换账号的确认挂在主窗口下面。
pub async fn confirm(app: &AppHandle, text: String, topmost: bool) -> bool {
    let main = app
        .get_webview_window("main")
        .and_then(|window| window.hwnd().ok())
        .map(|hwnd| hwnd.0 as isize);
    let owner = if topmost { None } else { main };
    if topmost && let Some(main) = main {
        dialog::flash(main);
    }
    tauri::async_runtime::spawn_blocking(move || dialog::confirm(owner, &text, topmost))
        .await
        .unwrap_or(false)
}
