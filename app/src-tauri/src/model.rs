//! 应用的全部状态，以及把快照推送给界面（设计文档第 6.3 节）。

use std::collections::HashMap;
use std::error::Error;
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard};

use mhy_carry_api::ApiClient;
use mhy_carry_core::account::GameRole;
use mhy_carry_core::scan::ScanStatus;
use mhy_carry_core::settings::Settings;
use mhy_carry_core::store::{AccountStore, JsonFileStore};
use mhy_carry_platform::paths;
use tauri::async_runtime::JoinHandle;
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::mpsc;

use crate::state::{AccountsView, AppState, BindView, Credential, SettingsView};

/// 推送状态快照的事件名（设计文档第 9 节）。
pub const STATE_EVENT: &str = "state-changed";

pub struct Model {
    pub accounts: AccountsView,
    /// 这次运行里查过的凭证状态，按账号的本地 ID 记。打开程序时不联网，一开始都是未检查。
    pub credentials: HashMap<String, Credential>,
    pub bind: BindView,
    pub scan: ScanStatus,
    /// 正在扫描用的账号的本地 ID。凭证状态和查到的原神角色都记到这个账号上。
    pub scan_account: Option<String>,
    pub settings: Settings,
    pub leftovers: usize,
    /// 正在进行的绑定，取消时中止它。
    pub bind_task: Option<JoinHandle<()>>,
    /// 正在进行的扫描的“停止”信号。扫描结束后清掉。
    pub scan_stop: Option<mpsc::Sender<()>>,
    /// 最近一条提示和它出现的时间（本地时间）。
    pub last_notice: Option<String>,
    pub notice_at: Option<String>,
    /// 最新快照的序号，每推送一次加一。
    seq: u64,
    /// 上一次推送给界面的快照，没变就不再推送。
    last_sent: Option<AppState>,
}

impl Model {
    pub fn scanning(&self) -> bool {
        self.scan_stop.is_some()
    }

    /// 账号相关的操作只能在空闲时做：扫描中、绑定中都不行，账号文件读不了时也不行
    /// （设计文档第 9 节）。
    pub fn ensure_idle(&self) -> Result<(), String> {
        if self.scanning() {
            return Err("扫描中不能修改账号，先停止扫描。".to_owned());
        }
        if self.bind.in_progress() {
            return Err("绑定中不能修改账号。".to_owned());
        }
        if let AccountsView::Unavailable { message } = &self.accounts {
            return Err(message.clone());
        }
        Ok(())
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
            accounts: AccountsView::None { notice: None },
            credentials: HashMap::new(),
            bind: BindView::Idle,
            scan: ScanStatus::Idle { last: None },
            scan_account: None,
            settings,
            leftovers: 0,
            bind_task: None,
            scan_stop: None,
            last_notice: None,
            notice_at: None,
            seq: 0,
            last_sent: None,
        };
        reload_accounts(&mut model, &store);
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
        seq: model.seq,
        accounts: model.accounts.clone(),
        bind: model.bind.clone(),
        scan: model.scan.clone(),
        scan_text: model.scan.to_string(),
        notice_text: model.scan.notice().map(ToString::to_string),
        notice_at: model.notice_at.clone(),
        settings: SettingsView {
            default_target: model.settings.scan.default_target,
            idle_timeout_secs: model.settings.scan.idle_timeout_secs,
        },
        credential_file: store.path().display().to_string(),
        leftovers: model.leftovers,
    }
}

/// 修改状态，有变化就把新的完整快照推送给界面。
///
/// 后台任务的推送要先排进事件循环，命令里的推送立刻发出，两者到达界面的顺序可能和生成
/// 顺序相反。所以每份新快照都带递增的序号，界面丢掉比手上旧的快照。
pub fn update(app: &AppHandle, change: impl FnOnce(&mut Model)) {
    let shared = app.state::<Shared>();
    let snapshot = {
        let mut model = shared.lock();
        change(&mut model);
        // 序号还没加，内容一样就和上一份完全相同，不用推送。
        let mut snapshot = snapshot_of(&model, &shared.store);
        if model.last_sent.as_ref() == Some(&snapshot) {
            return;
        }
        model.seq += 1;
        snapshot.seq = model.seq;
        model.last_sent = Some(snapshot.clone());
        snapshot
    };
    let _ = app.emit(STATE_EVENT, snapshot);
}

/// 重新读账号文件，刷新账号列表和残留文件数。
pub fn reload_accounts(model: &mut Model, store: &JsonFileStore) {
    model.accounts = AccountsView::from_load(
        store.load(),
        model.settings.active_account.as_deref(),
        &model.credentials,
    );
    model.leftovers = store.leftover_files().map_or(0, |files| files.len());
}

/// 把查到的原神角色存进账号文件，再刷新账号列表。写不进去也不影响使用，下次查到时再存
/// （设计文档第 6.4 节）。
pub fn save_role(model: &mut Model, store: &JsonFileStore, id: &str, role: &GameRole) {
    if let Ok(mut file) = store.load()
        && let Some(account) = file.get_mut(id)
        && account.game_role.as_ref() != Some(role)
    {
        account.game_role = Some(role.clone());
        let _ = store.save(&file);
    }
    reload_accounts(model, store);
}
