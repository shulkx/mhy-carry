//! MhyCarry 图形界面（设计文档第 6.3 节、第 9 节）。网页端只负责显示和点击，
//! 凭证、确认框、截图和联网都在 Rust 端。
// 正式版不显示控制台窗口。
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod bind;
mod commands;
mod model;
mod scan;
mod state;

use tauri::Manager;

fn main() {
    let result = tauri::Builder::default()
        .setup(|app| {
            app.manage(model::Shared::load()?);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_state,
            commands::bind_start,
            commands::bind_cancel,
            commands::unbind,
            commands::set_active,
            commands::set_note,
            commands::delete_leftovers,
            commands::list_targets,
            commands::scan_start,
            commands::scan_stop,
            commands::update_settings,
        ])
        .run(tauri::generate_context!());
    // 正式版没有控制台，启动失败（比如数据目录不可用、设置文件读写出错）时弹窗说明原因。
    if let Err(error) = result {
        mhy_carry_platform::dialog::alert(&format!("MhyCarry 启动失败：{error}"));
    }
}

#[cfg(test)]
mod tests {
    /// 数据目录名必须等于应用标识，卸载时勾选“删除应用程序数据”才会删掉它（设计文档第 8 节）。
    #[test]
    fn data_dir_name_is_the_app_identifier() {
        let config: serde_json::Value =
            serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
        assert_eq!(
            config["identifier"],
            mhy_carry_platform::paths::APP_DIR_NAME
        );
    }
}
