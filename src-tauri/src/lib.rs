//! VCE (Visual Command Environment): a PowerShell terminal with a synced
//! file tree, natural-language command search, error fix suggestions and an
//! on-demand local AI command generator.

mod ai;
mod assist;
mod env_path;
mod error;
mod fs_ops;
mod index;
mod settings;
mod shell;
mod system;

use tauri::{Manager, RunEvent};

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_log::Builder::new().level(log::LevelFilter::Info).max_file_size(2_000_000).build())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .manage(shell::ShellManager::default())
        .manage(index::IndexManager::default())
        .manage(ai::AiManager::default())
        .setup(|app| {
            let config_dir = app.path().app_config_dir()?;
            app.manage(settings::SettingsStore::load(config_dir.join("settings.json")));
            index::start_background_build(app.handle().clone(), false);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            shell::shell_attach,
            shell::shell_restart,
            shell::shell_write,
            shell::shell_resize,
            shell::shell_change_dir,
            shell::shell_run,
            shell::shell_insert,
            fs_ops::fs_list,
            fs_ops::fs_create_dir,
            fs_ops::fs_rename,
            fs_ops::fs_move,
            fs_ops::fs_trash,
            fs_ops::fs_open,
            fs_ops::fs_reveal,
            index::index_status,
            index::index_refresh,
            index::search_commands,
            assist::analyze_failure,
            assist::assess_risk,
            assist::fill_template,
            ai::ai_status,
            ai::ai_download_model,
            ai::ai_cancel_download,
            ai::ai_choose_model,
            ai::ai_use_default_model,
            ai::ai_generate,
            ai::ai_cancel,
            settings::settings_get,
            settings::settings_set_ui,
            system::system_screen_reader_active,
        ])
        .build(tauri::generate_context!())
        .expect("error while building the VCE application");

    app.run(|handle, event| {
        if let RunEvent::Exit = event {
            handle.state::<shell::ShellManager>().shutdown();
        }
    });
}
