//! 绑定账号：用米游社 App 扫码（设计文档第 6.1 节）。绑定的账号加进列表（已经在列表里就只
//! 更新凭证），设为使用中，再查一次原神角色（第 6.4 节）。

use std::time::Duration;

use mhy_carry_api::{ApiError, BindStatus, Credentials, retcode};
use mhy_carry_core::account::{Account, AccountFile, Upserted};
use mhy_carry_core::store::{AccountStore, StoreError};
use mhy_carry_runner::LoginApi;
use tauri::{AppHandle, Manager};

use crate::model::{Shared, reload_accounts, save_role, update};
use crate::state::{BindView, Credential, qr_data_uri};

const POLL_INTERVAL: Duration = Duration::from_secs(2);
/// 查询状态没有副作用，网络出错允许重试一次，连续两次就放弃（设计文档第 10 节）。
const MAX_NETWORK_ERRORS_IN_A_ROW: u32 = 2;
/// “绑定成功”显示这么久后自动收起（设计文档第 6.3 节）。
const SUCCESS_SHOWN_FOR: Duration = Duration::from_secs(5);

/// 跑完一次绑定，把结果写进状态。
pub async fn run(app: AppHandle) {
    let shared = app.state::<Shared>();
    let saved = match bind(&app).await {
        Ok(saved) => saved,
        Err(outcome) => {
            update(&app, |model| {
                // 已经被取消了就不再显示结果。
                if model.bind.in_progress() {
                    model.bind = outcome;
                }
            });
            return;
        }
    };
    update(&app, |model| {
        // 绑定成功的账号设为使用中。设置写不进去只影响下次启动时默认用哪个账号。
        model.settings.active_account = Some(saved.id.clone());
        let _ = model.settings.save(&shared.settings_path);
        // 刚拿到的凭证一定有效。
        model
            .credentials
            .insert(saved.id.clone(), Credential::Valid);
        reload_accounts(model, &shared.store);
        if model.bind.in_progress() {
            model.bind = BindView::Succeeded { added: saved.added };
        }
    });
    fetch_role(&app, &saved.id).await;
    // 成功时账号已经显示出来，提示几秒后收起；过期、取消、失败要你处理，一直显示。
    tokio::time::sleep(SUCCESS_SHOWN_FOR).await;
    update(&app, |model| {
        if matches!(model.bind, BindView::Succeeded { .. }) {
            model.bind = BindView::Idle;
        }
    });
}

/// 查一次原神角色，查到就存进账号文件。查不到不提示（设计文档第 6.4 节）。
async fn fetch_role(app: &AppHandle, id: &str) {
    let shared = app.state::<Shared>();
    let Ok(file) = shared.store.load() else {
        return;
    };
    let Some(account) = file.get(id) else {
        return;
    };
    let credentials = Credentials {
        stoken: &account.stoken,
        mid: &account.mid,
    };
    if let Ok(Some(role)) = shared
        .api
        .check_credential(&account.uid, &credentials)
        .await
    {
        update(app, |model| save_role(model, &shared.store, id, &role));
    }
}

async fn bind(app: &AppHandle) -> Result<Upserted, BindView> {
    let shared = app.state::<Shared>();
    let qr = shared
        .api
        .create_bind_qr()
        .await
        .map_err(|error| BindView::Failed {
            message: error.to_string(),
        })?;
    let image = qr_data_uri(&qr.url).map_err(|error| BindView::Failed {
        message: format!("生成二维码失败：{error}"),
    })?;
    // 取消只在下一个 await 处生效；这期间已经取消了，就不要把状态改回“进行中”。
    update(app, |model| {
        if model.bind.in_progress() {
            model.bind = BindView::WaitingScan { qr: image.clone() };
        }
    });

    let mut network_errors = 0;
    loop {
        tokio::time::sleep(POLL_INTERVAL).await;
        match shared.api.query_bind_qr(&qr.ticket).await {
            Ok(BindStatus::Created) => network_errors = 0,
            Ok(BindStatus::Scanned) => {
                network_errors = 0;
                update(app, |model| {
                    if model.bind.in_progress() {
                        model.bind = BindView::WaitingConfirm { qr: image.clone() };
                    }
                });
            }
            Ok(BindStatus::Confirmed(bound)) => {
                let account =
                    Account::new(bound.uid, bound.mid, bound.stoken, jiff::Timestamp::now());
                return save(&shared, account).map_err(|message| BindView::Failed { message });
            }
            Err(ApiError::Network(_)) if network_errors + 1 < MAX_NETWORK_ERRORS_IN_A_ROW => {
                network_errors += 1;
            }
            Err(error) if error.retcode() == Some(retcode::QR_EXPIRED) => {
                return Err(BindView::Expired);
            }
            Err(error) if error.retcode() == Some(retcode::QR_CANCELLED) => {
                return Err(BindView::Cancelled);
            }
            Err(error) => {
                return Err(BindView::Failed {
                    message: error.to_string(),
                });
            }
        }
    }
}

/// 保存账号：新的通行证加到列表末尾，已有的只更新凭证（设计文档第 6.1 节）。
fn save(shared: &Shared, account: Account) -> Result<Upserted, String> {
    let mut file = match shared.store.load() {
        Ok(file) => file,
        // 损坏的文件已经改名备份了，从空文件开始。
        Err(StoreError::Corrupt { .. }) => AccountFile::empty(),
        Err(error) => return Err(error.to_string()),
    };
    let saved = file.upsert(account);
    shared
        .store
        .save(&file)
        .map_err(|error| error.to_string())?;
    Ok(saved)
}
