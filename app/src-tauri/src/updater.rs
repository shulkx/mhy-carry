//! 检查更新和自动更新（设计文档第 6.6 节）。

use std::path::Path;
use std::time::Duration;

use mhy_carry_update::{Release, UpdateClient, UpdateError, Version, install};
use tauri::{AppHandle, Manager};

use crate::model::{Shared, update};
use crate::state::UpdateView;

/// 当前版本。版本号不是三段数字时（比如以后的预发布版本）不检查更新。
pub fn current_version() -> Option<Version> {
    Version::parse(env!("CARGO_PKG_VERSION"))
}

/// 打开程序时：删掉上次更新留下的旧版本；设置里开着就查一次新版本。
pub fn on_start(app: &AppHandle) {
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
    if app.state::<Shared>().lock().settings.update.check_on_start {
        start_check(app, false);
    }
}

/// 开始检查更新。已经在检查、下载或替换时不再查。`manual` 为 false（打开程序时）时，
/// 查失败了、没有新版本都不提示。
pub fn start_check(app: &AppHandle, manual: bool) {
    let Some(current) = current_version() else {
        return;
    };
    let mut start = false;
    update(app, |model| {
        if !model.update.busy() && model.update != UpdateView::Checking {
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
pub async fn install(app: AppHandle, version: Version) {
    let label = version.to_string();
    let failed = |message: String| UpdateView::Failed {
        message,
        version: Some(label.clone()),
    };
    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(error) => {
            let message = format!("找不到程序文件：{error}");
            return update(&app, |model| model.update = failed(message));
        }
    };
    let progress = |done: u64, total: Option<u64>| {
        let percent = total
            .filter(|&total| total > 0)
            .map_or(0, |total| (done.min(total) * 100 / total) as u8);
        update(&app, |model| {
            model.update = UpdateView::Downloading {
                version: label.clone(),
                percent,
            }
        });
    };
    if let Err(error) = download(version, &exe, progress).await {
        return update(&app, |model| model.update = failed(error.to_string()));
    }
    update(&app, |model| {
        model.update = UpdateView::Installing {
            version: label.clone(),
        }
    });
    if let Err(error) = install::swap(&exe) {
        let message = format!("换不上新版本：{error}。可以到发布页面手动下载。");
        return update(&app, |model| model.update = failed(message));
    }
    // 安装版：“设置 → 应用”里的版本号跟着变。改不了也不影响使用。
    if let Some(dir) = exe.parent() {
        let _ = mhy_carry_platform::install::record_version(dir, &label);
    }
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
    version: Version,
    exe: &Path,
    progress: impl FnMut(u64, Option<u64>),
) -> Result<(), UpdateError> {
    let client = UpdateClient::new()?;
    mhy_carry_update::download(&client, version, exe, progress).await?;
    Ok(())
}
