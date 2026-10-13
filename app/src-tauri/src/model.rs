//! 应用的全部状态，以及把快照推送给界面（设计文档第 6.3 节）。

use std::error::Error;
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard};

use mhy_carry_api::ApiClient;
use mhy_carry_core::scan::ScanStatus;
use mhy_carry_core::settings::Settings;
use mhy_carry_core::store::{AccountStore, JsonFileStore};
use mhy_carry_platform::paths;
use tauri::async_runtime::JoinHandle;
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::mpsc;

use crate::state::{AccountView, AppState, BindView, SettingsView};

/// 推送状态快照的事件名（设计文档第 9 节）。
pub const STATE_EVENT: &str = "state-changed";

pub struct Model {
    pub account: AccountView,
    pub bind: BindView,
    pub scan: ScanStatus,
    pub settings: Settings,
    pub leftovers: usize,
    /// 正在进行的绑定，取消时中止它。
    pub bind_task: Option<JoinHandle<()>>,
    /// 正在进行的扫描的“停止”信号。扫描结束后清掉。
    pub scan_stop: Option<mpsc::Sender<()>>,
    /// 上一次推送给界面的快照，没变就不再推送。
    last_sent: Option<AppState>,
}

impl Model {
    pub fn scanning(&self) -> bool {
        self.scan_stop.is_some()
    }
}

/// 交给 Tauri 管理的共享状态。
pub struct Shared {
    model: Mutex<Model>,
    pub store: JsonFileStore,
    pub settings_path: PathBuf,
    pub api: ApiClient,
}

impl Shared {
    /// 启动时读取设置和账号文件。打开程序时不联网（设计文档第 6.3 节）。
    pub fn load() -> Result<Self, Box<dyn Error>> {
        let settings_path = paths::settings_path()?;
        let settings = Settings::load_or_init(&settings_path)?;
        let store = JsonFileStore::new(paths::accounts_path()?);
        let api = ApiClient::new(settings.device_id.clone())?;
        let mut model = Model {
            account: AccountView::None { notice: None },
            bind: BindView::Idle,
            scan: ScanStatus::Idle { last: None },
            settings,
            leftovers: 0,
            bind_task: None,
            scan_stop: None,
            last_sent: None,
        };
        reload_account(&mut model, &store);
        Ok(Self {
            model: Mutex::new(model),
            store,
            settings_path,
            api,
        })
    }

    pub fn lock(&self) -> MutexGuard<'_, Model> {
        // 锁只在同步代码里短暂持有；万一中毒了，照常用里面的数据。
        self.model
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub fn snapshot(&self) -> AppState {
        snapshot_of(&self.lock(), &self.store)
    }
}

fn snapshot_of(model: &Model, store: &JsonFileStore) -> AppState {
    AppState {
        account: model.account.clone(),
        bind: model.bind.clone(),
        scan: model.scan.clone(),
        scan_text: model.scan.to_string(),
        notice_text: model.scan.notice().map(ToString::to_string),
        settings: SettingsView {
            default_target: model.settings.scan.default_target,
            idle_timeout_secs: model.settings.scan.idle_timeout_secs,
        },
        credential_file: store.path().display().to_string(),
        leftovers: model.leftovers,
    }
}

/// 修改状态，有变化就把新的完整快照推送给界面。
pub fn update(app: &AppHandle, change: impl FnOnce(&mut Model)) {
    let shared = app.state::<Shared>();
    let snapshot = {
        let mut model = shared.lock();
        change(&mut model);
        let snapshot = snapshot_of(&model, &shared.store);
        if model.last_sent.as_ref() == Some(&snapshot) {
            return;
        }
        model.last_sent = Some(snapshot.clone());
        snapshot
    };
    let _ = app.emit(STATE_EVENT, snapshot);
}

/// 重新读账号文件，刷新账号状态和残留文件数。
pub fn reload_account(model: &mut Model, store: &JsonFileStore) {
    model.account = AccountView::from_load(store.load());
    model.leftovers = store.leftover_files().map_or(0, |files| files.len());
}
