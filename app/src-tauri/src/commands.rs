//! 网页端能调用的命令（设计文档第 9 节）。每个命令都要在 capabilities/main.json 里单独授权。
//! 返回的错误是给人看的中文句子。

use std::time::Duration;

use mhy_carry_core::account::{Account, normalize_note};
use mhy_carry_core::capture::FrameSource;
use mhy_carry_core::decode::RxingDecoder;
use mhy_carry_core::games::GAMES;
use mhy_carry_core::room::parse_room;
use mhy_carry_core::settings::TargetKind;
use mhy_carry_core::store::AccountStore;
use mhy_carry_core::video::H264Decoder;
use mhy_carry_live::LiveSource;
use mhy_carry_platform::capture::{
    GameWindowSource, MonitorSource, PickedWindowSource, list_monitors, list_windows,
};
use mhy_carry_platform::video::MfH264Decoder;
use mhy_carry_runner::ScanJob;
use serde::Serialize;
use tauri::{AppHandle, Manager, State};
use tokio::sync::mpsc;

use crate::model::{Model, Shared, reload_accounts, update};
use crate::state::{AppState, BindView, apply_settings};
use crate::{bind, scan};

type CommandResult<T = ()> = Result<T, String>;

const ACCOUNT_GONE: &str = "这个账号已经不在了。";

#[tauri::command]
pub fn get_state(shared: State<'_, Shared>) -> AppState {
    shared.snapshot()
}

/// 添加账号，或者重新绑定已有的账号（设计文档第 6.1 节）。扫码时登录的是哪个账号，就添加或
/// 更新哪个，不会替换别的账号，所以不用确认。
#[tauri::command]
pub fn bind_start(app: AppHandle) -> CommandResult {
    let mut result = Ok(());
    let mut start = false;
    update(&app, |model| {
        if model.bind.in_progress() {
            return;
        }
        match model.ensure_idle() {
            Ok(()) => {
                model.bind = BindView::Requesting;
                start = true;
            }
            Err(message) => result = Err(message),
        }
    });
    if start {
        let task = tauri::async_runtime::spawn(bind::run(app.clone()));
        // 上一次绑定成功后可能还在等着收起提示，停掉它，免得收起这一次的结果。
        if let Some(previous) = app.state::<Shared>().lock().bind_task.replace(task) {
            previous.abort();
        }
    }
    result
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

/// 切换使用中的账号（设计文档第 6.4 节）。
#[tauri::command]
pub fn set_active(app: AppHandle, id: String) -> CommandResult {
    let shared = app.state::<Shared>();
    let mut result = Ok(());
    update(&app, |model| {
        result = activate(model, &shared, &id);
        reload_accounts(model, &shared.store);
    });
    result
}

fn activate(model: &mut Model, shared: &Shared, id: &str) -> CommandResult {
    model.ensure_idle()?;
    if !model.accounts.contains(id) {
        return Err(ACCOUNT_GONE.to_owned());
    }
    let previous = model.settings.active_account.replace(id.to_owned());
    if let Err(error) = model.settings.save(&shared.settings_path) {
        // 没保存成功就退回原来的设置，免得界面上显示的和重启后的不一样。
        model.settings.active_account = previous;
        return Err(error.to_string());
    }
    Ok(())
}

/// 改备注。最多 20 个字，清空就是去掉备注（设计文档第 6.4 节）。
#[tauri::command]
pub fn set_note(app: AppHandle, id: String, note: String) -> CommandResult {
    let shared = app.state::<Shared>();
    let mut result = Ok(());
    update(&app, |model| {
        result = write_note(model, &shared, &id, &note);
        reload_accounts(model, &shared.store);
    });
    result
}

fn write_note(model: &Model, shared: &Shared, id: &str, note: &str) -> CommandResult {
    model.ensure_idle()?;
    let mut file = shared.store.load().map_err(|error| error.to_string())?;
    file.get_mut(id).ok_or(ACCOUNT_GONE)?.note = normalize_note(note);
    shared.store.save(&file).map_err(|error| error.to_string())
}

/// 解绑一个账号，先用系统确认框问你（设计文档第 6.4 节）。
#[tauri::command]
pub async fn unbind(app: AppHandle, id: String) -> CommandResult {
    let label = {
        let shared = app.state::<Shared>();
        shared.lock().ensure_idle()?;
        let file = shared.store.load().map_err(|error| error.to_string())?;
        file.get(&id).map(Account::describe).ok_or(ACCOUNT_GONE)?
    };
    let question = format!(
        "解绑{label}？会删除本机保存的这个账号的凭证。\n\n\
         这不会让凭证在服务器端失效；需要的话，请在米游社 App 里下线设备或修改密码。"
    );
    if !scan::confirm(&app, question, false).await {
        return Ok(());
    }
    let shared = app.state::<Shared>();
    let mut result = Ok(());
    update(&app, |model| {
        result = remove_account(model, &shared, &id);
        reload_accounts(model, &shared.store);
    });
    result
}

fn remove_account(model: &mut Model, shared: &Shared, id: &str) -> CommandResult {
    model.ensure_idle()?;
    let mut file = shared.store.load().map_err(|error| error.to_string())?;
    file.remove(id).ok_or(ACCOUNT_GONE)?;
    shared
        .store
        .save(&file)
        .map_err(|error| error.to_string())?;
    model.credentials.remove(id);
    // 解绑的是使用中的账号时，改用列表里的第一个。设置写不进去也一样：记的账号不在了，
    // 读的时候就用第一个。
    if model.settings.active_account.as_deref() == Some(id) {
        model.settings.active_account = None;
        let _ = model.settings.save(&shared.settings_path);
    }
    Ok(())
}

/// 删除账号文件损坏时留下的旧文件。里面可能有明文凭证，先确认（设计文档第 6.4 节）。
#[tauri::command]
pub async fn delete_leftovers(app: AppHandle) -> CommandResult {
    let count = {
        let shared = app.state::<Shared>();
        if shared.lock().scanning() {
            return Err("扫描中不能删除，先停止扫描。".to_owned());
        }
        shared
            .store
            .leftover_files()
            .map_err(|error| error.to_string())?
            .len()
    };
    if count == 0 {
        return Ok(());
    }
    let question = format!("删除账号文件损坏时留下的 {count} 个旧文件？里面可能有明文凭证。");
    if !scan::confirm(&app, question, false).await {
        return Ok(());
    }
    let shared = app.state::<Shared>();
    let result = remove_leftovers(&shared);
    update(&app, |model| reload_accounts(model, &shared.store));
    result
}

fn remove_leftovers(shared: &Shared) -> CommandResult {
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
        TargetKind::Auto | TargetKind::Live => Vec::new(),
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

/// 用使用中的账号开始扫描（设计文档第 6.2、6.4、6.5 节）。`room` 只在直播间时用，
/// 会记进设置，下次自动填上。
#[tauri::command(rename_all = "snake_case")]
pub async fn scan_start(
    app: AppHandle,
    target: TargetKind,
    id: Option<u32>,
    room: Option<String>,
) -> CommandResult {
    let (stop, stop_signal) = mpsc::channel(1);
    let (idle_timeout, active, auto_confirm) = {
        let shared = app.state::<Shared>();
        let mut model = shared.lock();
        if model.scanning() {
            return Ok(());
        }
        if model.bind.in_progress() {
            return Err("绑定中不能开始扫描。".to_owned());
        }
        if target == TargetKind::Live {
            let room = room.as_deref().unwrap_or_default();
            parse_room(room).map_err(|error| error.to_string())?;
            // 记住这次的直播间。写不进去也不影响这次扫描。
            model.settings.scan.live_room = room.trim().to_owned();
            let _ = model.settings.save(&shared.settings_path);
        }
        model.scan_stop = Some(stop);
        model.last_notice = None;
        model.notice_at = None;
        (
            model.settings.scan.idle_timeout(),
            model.settings.active_account.clone(),
            model.settings.scan.auto_confirm,
        )
    };
    let job = prepare_job(
        &app,
        target,
        id,
        room.as_deref(),
        idle_timeout,
        active.as_deref(),
    );
    match job {
        Ok((mut job, account_id)) => {
            job.auto_confirm = auto_confirm;
            app.state::<Shared>().lock().scan_account = Some(account_id);
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
    room: Option<&str>,
    idle_timeout: Duration,
    active: Option<&str>,
) -> CommandResult<(ScanJob, String)> {
    let shared = app.state::<Shared>();
    let mut file = shared.store.load().map_err(|error| error.to_string())?;
    let account_id = file
        .active(active)
        .map(|account| account.id.clone())
        .ok_or("还没有绑定账号，先用米游社扫码绑定。")?;
    let account = file.remove(&account_id).ok_or(ACCOUNT_GONE)?;
    let source: Box<dyn FrameSource + Send> = match target {
        TargetKind::Auto => Box::new(GameWindowSource::new(GAMES)),
        TargetKind::Live => {
            let room = parse_room(room.unwrap_or_default()).map_err(|error| error.to_string())?;
            Box::new(
                LiveSource::start(room, Box::new(system_decoder))
                    .map_err(|error| error.to_string())?,
            )
        }
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
    let job = ScanJob {
        source,
        decoder: Box::new(RxingDecoder),
        account: account.describe(),
        uid: account.uid,
        mid: account.mid,
        stoken: account.stoken,
        idle_timeout,
        auto_confirm: false,
    };
    Ok((job, account_id))
}

/// 在拉流线程里创建系统 H.264 解码器。
fn system_decoder() -> Result<Box<dyn H264Decoder>, String> {
    MfH264Decoder::new().map(|decoder| Box::new(decoder) as Box<dyn H264Decoder>)
}

/// 打开或关闭自动确认，记在设置里（设计文档第 6.5 节）。扫描中不能改：这次扫描的策略开始时就定了。
#[tauri::command]
pub fn set_auto_confirm(app: AppHandle, on: bool) -> CommandResult {
    let shared = app.state::<Shared>();
    let mut result = Ok(());
    update(&app, |model| {
        if model.scanning() {
            result = Err("扫描中不能改，先停止扫描。".to_owned());
            return;
        }
        let previous = model.settings.scan.auto_confirm;
        model.settings.scan.auto_confirm = on;
        if let Err(error) = model.settings.save(&shared.settings_path) {
            model.settings.scan.auto_confirm = previous;
            result = Err(error.to_string());
        }
    });
    result
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
        model.settings.scan = apply_settings(&previous, default_target, idle_timeout_secs);
        if let Err(error) = model.settings.save(&shared.settings_path) {
            // 没保存成功就退回原来的设置，免得界面上显示的和重启后的不一样。
            model.settings.scan = previous;
            result = Err(error.to_string());
        }
    });
    result
}
