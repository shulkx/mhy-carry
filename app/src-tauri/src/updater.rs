//! 检查更新和自动更新（设计文档第 6.6 节）。

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use mhy_carry_update::{Release, Staged, UpdateClient, UpdateError, Version, install};
use tauri::{AppHandle, Manager};

use crate::model::{Shared, update};
use crate::state::UpdateView;

/// 当前版本。版本号不是三段数字时（比如以后的预发布版本）不检查更新。
pub fn current_version() -> Option<Version> {
    Version::parse(env!("CARGO_PKG_VERSION"))
}

/// 打开程序时：设置里开着就查一次新版本。
pub fn on_start(app: &AppHandle) {
    if app.state::<Shared>().lock().settings.update.check_on_start {
        start_check(app, false);
    }
}

/// 界面第一次取快照时调用：新版本已经正常打开了，这时才删掉上次更新换下来的旧版本，
/// 免得新版本一打开就坏掉时没法退回（第 6.6 节“更新”第 6 步）。
pub fn clean_up_once() {
    static DONE: AtomicBool = AtomicBool::new(false);
    if DONE.swap(true, Ordering::Relaxed) {
        return;
    }
    if let Ok(exe) = std::env::current_exe() {
        // 旧进程可能还没退出，删不掉时过一秒再试，最多 10 秒。
        std::thread::spawn(move || {
            for _ in 0..10 {
                if install::clean_up(&exe) {
                    break;
                }
                std::thread::sleep(Duration::from_secs(1));
            }
        });
    }
}

/// 开始检查更新。检查中、下载或替换中、等着重启时都不再查。`manual` 为 false（打开程序时）时，
/// 查失败了、没有新版本都不提示。
pub fn start_check(app: &AppHandle, manual: bool) {
    let Some(current) = current_version() else {
        return;
    };
    let mut start = false;
    update(app, |model| {
        if model.update.can_check() {
            model.update = UpdateView::Checking;
            start = true;
        }
    });
    if start {
        tauri::async_runtime::spawn(check(app.clone(), current, manual));
    }
}

async fn check(app: AppHandle, current: Version, manual: bool) {
    let result = latest(current).await;
    update(&app, |model| {
        model.update = match result {
            Ok(Some(release)) => UpdateView::Available {
                version: release.version.to_string(),
                installable: release.installable,
            },
            Ok(None) if manual => UpdateView::Latest,
            Err(error) if manual => UpdateView::Failed {
                message: error.to_string(),
                version: None,
            },
            _ => UpdateView::Idle,
        };
    });
}

async fn latest(current: Version) -> Result<Option<Release>, UpdateError> {
    mhy_carry_update::check(&UpdateClient::new()?, current).await
}

/// 下载、核对、换上 `version`，然后重启。调用前状态已经设成下载中。
/// 下载中可以取消：状态会退回“有新版本”，之后的进度和结果都不再写回去。
pub async fn install(app: AppHandle, version: Version) {
    let label = version.to_string();
    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(error) => {
            let message = format!("找不到程序文件：{error}");
            return finish_download(&app, &label, message);
        }
    };
    let staged = match download(&app, version, &label, &exe).await {
        Ok(staged) => staged,
        Err(error) => return finish_download(&app, &label, error.to_string()),
    };
    let mut proceed = false;
    update(&app, |model| {
        if downloading(&model.update, &label) {
            model.update = UpdateView::Installing {
                version: label.clone(),
            };
            model.update_task = None;
            proceed = true;
        }
    });
    if !proceed {
        return;
    }
    if let Err(error) = staged.install() {
        return update(&app, |model| {
            model.update = failed(&label, error.to_string())
        });
    }
    // 安装版：“设置 → 应用”里的版本号跟着变。改不了也不影响使用。
    if let Some(dir) = exe.parent() {
        let _ = mhy_carry_platform::install::record_version(dir, &label);
    }
    // 先放开“只能开一个”的锁：新版本启动得比这个进程退出快，不放开的话它会以为已经有一个
    // 在运行，切过来以后自己退出，更新完程序就没了（第 6.6 节“更新”第 6 步）。
    tauri_plugin_single_instance::destroy(&app);
    match std::process::Command::new(&exe).spawn() {
        Ok(_) => app.exit(0),
        Err(_) => update(&app, |model| {
            model.update = UpdateView::NeedsRestart {
                version: label.clone(),
            }
        }),
    }
}

async fn download(
    app: &AppHandle,
    version: Version,
    label: &str,
    exe: &Path,
) -> Result<Staged, UpdateError> {
    let progress = |done: u64, total: Option<u64>| {
        let percent = total
            .filter(|&total| total > 0)
            .map_or(0, |total| (done.min(total) * 100 / total) as u8);
        update(app, |model| {
            if downloading(&model.update, label) {
                model.update = UpdateView::Downloading {
                    version: label.to_owned(),
                    percent,
                };
            }
        });
    };
    let client = UpdateClient::new()?;
    mhy_carry_update::download(&client, version, exe, progress).await
}

/// 还在下载 `version`。取消后就不是了。
fn downloading(update: &UpdateView, version: &str) -> bool {
    matches!(update, UpdateView::Downloading { version: current, .. } if current == version)
}

/// 下载没成功：还没被取消的话，显示失败原因。
fn finish_download(app: &AppHandle, version: &str, message: String) {
    update(app, |model| {
        if downloading(&model.update, version) {
            model.update = failed(version, message);
            model.update_task = None;
        }
    });
}

fn failed(version: &str, message: String) -> UpdateView {
    UpdateView::Failed {
        message,
        version: Some(version.to_owned()),
    }
}
