#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod adb;
mod agent;
mod cluster;
mod gpu;
mod lan;
mod llama;
mod settings;
mod split;
mod state;

use std::sync::Arc;
use tauri::Manager;

/// versionName of the bundled node.apk. resources/node.version (one line) overrides the default,
/// so a rebuilt APK can ship without recompiling the desktop app.
const DEFAULT_NODE_VERSION: &str = "0.2 (3)";

fn main() {
    tauri::Builder::default()
        .setup(|app| {
            let res = app.path().resource_dir().ok().map(|d| d.join("resources"));
            let adb_bin = adb::resolve_bin(res.as_deref(), "adb");
            let llama_bin = adb::resolve_bin(res.as_deref(), "llama-server");
            let apk = res.as_ref().map(|d| d.join("node.apk"));
            let node_version = res.as_ref()
                .and_then(|d| std::fs::read_to_string(d.join("node.version")).ok())
                .map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
                .unwrap_or_else(|| DEFAULT_NODE_VERSION.to_string());
            let config = app.path().app_config_dir().ok().map(|d| d.join("argus-settings.json"));

            let state = Arc::new(state::AppState::new(adb::Adb::new(adb_bin), apk, node_version, llama_bin, config));
            app.manage(state.clone());
            tauri::async_runtime::spawn(cluster::run_poll_loop(app.handle().clone(), state));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            cluster::get_snapshot,
            cluster::rescan,
            llama::start_model,
            llama::stop_model,
            llama::model_status,
            lan::add_lan_node,
            lan::remove_lan_node,
            lan::test_lan_node,
            lan::set_node_weight,
            lan::set_node_enabled,
            lan::set_fast_start,
            agent::agent_run_command,
            agent::agent_read_file,
            agent::agent_write_file,
            agent::agent_list_dir,
            agent::agent_screenshot,
            agent::agent_mouse,
            agent::agent_type_text,
            agent::agent_key_press,
            agent::open_external,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
