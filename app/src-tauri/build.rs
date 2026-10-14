fn main() {
    // 网页端只能调用这里列出的命令，而且每个命令还要在 capabilities/main.json 里单独授权
    // （设计文档第 9 节）。
    let manifest = tauri_build::AppManifest::new().commands(&[
        "get_state",
        "bind_start",
        "bind_cancel",
        "unbind",
        "set_active",
        "set_note",
        "delete_leftovers",
        "list_targets",
        "scan_start",
        "scan_stop",
        "update_settings",
    ]);
    tauri_build::try_build(tauri_build::Attributes::new().app_manifest(manifest))
        .expect("tauri-build 失败");
}
