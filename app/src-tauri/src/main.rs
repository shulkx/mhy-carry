//! MhyCarry 图形界面（设计文档第 6.3 节、第 9 节）。网页端只负责显示和点击，
//! 凭证、确认框、截图和联网都在 Rust 端。
// 正式版不显示控制台窗口。
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
// 任务 7 接上命令后删除。
#![allow(dead_code)]

mod model;
mod state;

use tauri::Manager;

fn main() {
    tauri::Builder::default()
        .setup(|app| {
            app.manage(model::Shared::load()?);
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("启动 MhyCarry 失败");
}
