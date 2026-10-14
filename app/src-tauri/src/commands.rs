//! 网页端能调用的命令（设计文档第 9 节）。每个命令都要在 capabilities/main.json 里单独授权。
//! 返回的错误是给人看的中文句子。

use std::time::Duration;

use mhy_carry_core::account::AccountFile;
use mhy_carry_core::capture::FrameSource;
use mhy_carry_core::decode::RxingDecoder;
use mhy_carry_core::games::GAMES;
use mhy_carry_core::settings::TargetKind;
use mhy_carry_core::store::AccountStore;
use mhy_carry_platform::capture::{
    GameWindowSource, MonitorSource, PickedWindowSource, list_monitors, list_windows,
};
use mhy_carry_runner::ScanJob;
use serde::Serialize;
use tauri::{AppHandle, Manager, State};
use tokio::sync::mpsc;

use crate::model::{Shared, reload_account, update};
use crate::state::{AccountView, AppState, BindView, validated_settings};
use crate::{bind, scan};

type CommandResult<T = ()> = Result<T, String>;

#[tauri::command]
pub fn get_state(shared: State<'_, Shared>) -> AppState {
    shared.snapshot()
}

#[tauri::command]
pub async fn bind_start(app: AppHandle) -> CommandResult {
    let existing = {
        let shared = app.state::<Shared>();
        let model = shared.lock();
        if model.scanning() {
            return Err("扫描中不能绑定，先停止扫描。".to_owned());
        }
        if model.bind.in_progress() {
            return Ok(());
        }
        match &model.account {
            AccountView::Bound { uid, .. } => Some(uid.clone()),
            AccountView::Unavailable { message } => return Err(message.clone()),
            AccountView::None { .. } => None,
        }
    };
    if let Some(uid) = existing {
        let question = format!(
            "已经绑定了账号 {uid}，要换成新账号吗？新账号绑定成功后，旧账号的凭证会被删除。"
        );
        if !scan::confirm(&app, question, false).await {
            return Ok(());
        }
    }
    let mut start = false;
    update(&app, |model| {
        if !model.scanning() && !model.bind.in_progress() {
            model.bind = BindView::Requesting;
            start = true;
        }
    });
    if start {
        let task = tauri::async_runtime::spawn(bind::run(app.clone()));
        // 上一次绑定成功后可能还在等着收起提示，停掉它，免得收起这一次的结果。
        if let Some(previous) = app.state::<Shared>().lock().bind_task.replace(task) {
            previous.abort();
        }
    }
    Ok(())
}

/// 取消正在进行的绑定，或者关掉绑定结果。
#[tauri::command]
pub fn bind_cancel(app: AppHandle) {
    update(&app, |model| {
        if let Some(task) = model.bind_task.take() {
            task.abort();
        }
        model.bind = BindView::Idle;
    });
}

#[tauri::command]
pub async fn unbind(app: AppHandle) -> CommandResult {
    let bound = {
        let shared = app.state::<Shared>();
        let model = shared.lock();
        if model.scanning() {
            return Err("扫描中不能解绑，先停止扫描。".to_owned());
        }
        match &model.account {
            AccountView::Unavailable { message } => return Err(message.clone()),
            AccountView::Bound { .. } => true,
            AccountView::None { .. } => false,
        }
    };
    // 未绑定时，界面上的“删除旧账号文件”也走这里，只删残留的旧文件。
    let question = if bound {
        "解绑会删除本机保存的凭证，包括账号文件损坏时留下的旧文件。确定吗？\n\n\
         这不会让凭证在服务器端失效；需要的话，请在米游社 App 里下线设备或修改密码。"
            .to_owned()
    } else {
        "删除账号文件损坏时留下的旧文件？里面可能有明文凭证。".to_owned()
    };
    if !scan::confirm(&app, question, false).await {
        return Ok(());
    }
    let shared = app.state::<Shared>();
    let result = remove_credentials(&shared);
    update(&app, |model| reload_account(model, &shared.store));
    result
}

fn remove_credentials(shared: &Shared) -> CommandResult {
    if matches!(shared.store.load(), Ok(file) if !file.accounts.is_empty()) {
        shared
            .store
            .save(&AccountFile::empty())
            .map_err(|error| error.to_string())?;
    }
    for path in shared
        .store
        .leftover_files()
        .map_err(|error| error.to_string())?
    {
        std::fs::remove_file(&path)
            .map_err(|error| format!("删除 {} 失败：{error}", path.display()))?;
    }
    Ok(())
}

/// 可以选来扫描的窗口或显示器。
#[derive(Serialize)]
pub struct TargetView {
    id: u32,
    label: String,
    /// 默认选中它：游戏窗口、主显示器。
    preferred: bool,
}

#[tauri::command]
pub fn list_targets(kind: TargetKind) -> CommandResult<Vec<TargetView>> {
    Ok(match kind {
        TargetKind::Auto => Vec::new(),
        TargetKind::Window => list_windows()
            .map_err(|error| error.to_string())?
            .into_iter()
            .map(|window| TargetView {
                id: window.id,
                label: window.label,
                preferred: window.is_game,
            })
            .collect(),
        TargetKind::Monitor => list_monitors()
            .map_err(|error| error.to_string())?
            .into_iter()
            .map(|monitor| TargetView {
                id: monitor.id,
                label: if monitor.is_primary {
                    format!("{}（主显示器）", monitor.label)
                } else {
                    monitor.label
                },
                preferred: monitor.is_primary,
            })
            .collect(),
    })
}

#[tauri::command(rename_all = "snake_case")]
pub async fn scan_start(app: AppHandle, target: TargetKind, id: Option<u32>) -> CommandResult {
    let (stop, stop_signal) = mpsc::channel(1);
    let idle_timeout = {
        let shared = app.state::<Shared>();
        let mut model = shared.lock();
        if model.scanning() {
            return Ok(());
        }
        if model.bind.in_progress() {
            return Err("绑定中不能开始扫描。".to_owned());
        }
        model.scan_stop = Some(stop);
        model.last_notice = None;
        model.notice_at = None;
        model.settings.scan.idle_timeout()
    };
    match prepare_job(&app, target, id, idle_timeout) {
        Ok(job) => {
            scan::spawn(app, job, stop_signal);
            Ok(())
        }
        Err(message) => {
            update(&app, |model| model.scan_stop = None);
            Err(message)
        }
    }
}

fn prepare_job(
    app: &AppHandle,
    target: TargetKind,
    id: Option<u32>,
    idle_timeout: Duration,
) -> CommandResult<ScanJob> {
    let shared = app.state::<Shared>();
    let account = shared
        .store
        .load()
        .map_err(|error| error.to_string())?
        .accounts
        .into_iter()
        .next()
        .ok_or("还没有绑定账号，先用米游社扫码绑定。")?;
    let source: Box<dyn FrameSource + Send> = match target {
        TargetKind::Auto => Box::new(GameWindowSource::new(GAMES)),
        TargetKind::Window => {
            let id = id.ok_or("先选择要扫描的窗口。")?;
            let window = list_windows()
                .map_err(|error| error.to_string())?
                .into_iter()
                .find(|window| window.id == id)
                .ok_or("这个窗口已经不在了，请刷新列表。")?;
            Box::new(PickedWindowSource::new(&window))
        }
        TargetKind::Monitor => {
            let id = id.ok_or("先选择要扫描的显示器。")?;
            let monitor = list_monitors()
                .map_err(|error| error.to_string())?
                .into_iter()
                .find(|monitor| monitor.id == id)
                .ok_or("这个显示器已经不在了，请刷新列表。")?;
            Box::new(MonitorSource::new(&monitor))
        }
    };
    Ok(ScanJob {
        source,
        decoder: Box::new(RxingDecoder),
        account: account.describe(),
        uid: account.uid,
        mid: account.mid,
        stoken: account.stoken,
        idle_timeout,
    })
}

#[tauri::command]
pub fn scan_stop(shared: State<'_, Shared>) {
    if let Some(stop) = &shared.lock().scan_stop {
        let _ = stop.try_send(());
    }
}

#[tauri::command(rename_all = "snake_case")]
pub fn update_settings(
    app: AppHandle,
    default_target: TargetKind,
    idle_timeout_secs: u64,
) -> CommandResult {
    let shared = app.state::<Shared>();
    let mut result = Ok(());
    update(&app, |model| {
        let previous = model.settings.scan.clone();
        model.settings.scan = validated_settings(default_target, idle_timeout_secs);
        if let Err(error) = model.settings.save(&shared.settings_path) {
            // 没保存成功就退回原来的设置，免得界面上显示的和重启后的不一样。
            model.settings.scan = previous;
            result = Err(error.to_string());
        }
    });
    result
}
