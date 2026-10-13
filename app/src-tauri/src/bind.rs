//! 绑定账号：用米游社 App 扫码（设计文档第 6.1 节）。

use std::time::Duration;

use mhy_carry_api::{ApiError, BindStatus, retcode};
use mhy_carry_core::account::{Account, AccountFile};
use mhy_carry_core::store::{AccountStore, StoreError};
use tauri::{AppHandle, Manager};

use crate::model::{Shared, reload_account, update};
use crate::state::{BindView, qr_data_uri};

const POLL_INTERVAL: Duration = Duration::from_secs(2);
/// 查询状态没有副作用，网络出错允许重试一次，连续两次就放弃（设计文档第 10 节）。
const MAX_NETWORK_ERRORS_IN_A_ROW: u32 = 2;

/// 跑完一次绑定，把结果写进状态。
pub async fn run(app: AppHandle) {
    let outcome = bind(&app).await;
    let shared = app.state::<Shared>();
    update(&app, |model| {
        if matches!(outcome, BindView::Succeeded { .. }) {
            reload_account(model, &shared.store);
        }
        model.bind = outcome;
    });
}

async fn bind(app: &AppHandle) -> BindView {
    let shared = app.state::<Shared>();
    let qr = match shared.api.create_bind_qr().await {
        Ok(qr) => qr,
        Err(error) => {
            return BindView::Failed {
                message: error.to_string(),
            };
        }
    };
    let image = match qr_data_uri(&qr.url) {
        Ok(image) => image,
        Err(error) => {
            return BindView::Failed {
                message: format!("生成二维码失败：{error}"),
            };
        }
    };
    update(app, |model| {
        model.bind = BindView::WaitingScan { qr: image.clone() }
    });

    let mut network_errors = 0;
    loop {
        tokio::time::sleep(POLL_INTERVAL).await;
        match shared.api.query_bind_qr(&qr.ticket).await {
            Ok(BindStatus::Created) => network_errors = 0,
            Ok(BindStatus::Scanned) => {
                network_errors = 0;
                update(app, |model| {
                    model.bind = BindView::WaitingConfirm { qr: image.clone() }
                });
            }
            Ok(BindStatus::Confirmed(bound)) => {
                let uid = bound.uid.clone();
                let account =
                    Account::new(bound.uid, bound.mid, bound.stoken, jiff::Timestamp::now());
                return match save(&shared, account) {
                    Ok(()) => BindView::Succeeded { uid },
                    Err(message) => BindView::Failed { message },
                };
            }
            Err(ApiError::Network(_)) if network_errors + 1 < MAX_NETWORK_ERRORS_IN_A_ROW => {
                network_errors += 1;
            }
            Err(error) if error.retcode() == Some(retcode::QR_EXPIRED) => return BindView::Expired,
            Err(error) if error.retcode() == Some(retcode::QR_CANCELLED) => {
                return BindView::Cancelled;
            }
            Err(error) => {
                return BindView::Failed {
                    message: error.to_string(),
                };
            }
        }
    }
}

/// 保存账号。v1 只支持一个账号，新账号替换旧的（设计文档第 6.1 节）。
fn save(shared: &Shared, account: Account) -> Result<(), String> {
    let mut file = match shared.store.load() {
        Ok(file) => file,
        // 损坏的文件已经改名备份了，从空文件开始。
        Err(StoreError::Corrupt { .. }) => AccountFile::empty(),
        Err(error) => return Err(error.to_string()),
    };
    file.accounts = vec![account];
    shared.store.save(&file).map_err(|error| error.to_string())
}
