#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod adb;
mod agent;
mod beacon;
mod cluster;
mod gpu;
mod lan;
mod llama;
mod settings;
mod split;
mod state;
mod updater;

use std::sync::Arc;
use tauri::Manager;

/// "versionName (versionCode)" of the bundled node.apk. Read at run time from resources/node.version;
/// the fallback is the same file baked in at compile time, so it can never drift from the APK again.
pub fn default_node_version() -> &'static str { include_str!("../resources/node.version").trim() }

fn main() {
    let app = tauri::Builder::default()
        .setup(|app| {
            let res = app.path().resource_dir().ok().map(|d| d.join("resources"));
            let adb_bin = adb::resolve_bin(res.as_deref(), "adb");
            let llama_bin = adb::resolve_bin(res.as_deref(), "llama-server");
            let apk = res.as_ref().map(|d| d.join("node.apk"));
            let node_version = res.as_ref()
                .and_then(|d| std::fs::read_to_string(d.join("node.version")).ok())
                .map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
                .unwrap_or_else(|| default_node_version().to_string());
            let config = app.path().app_config_dir().ok().map(|d| d.join("argus-settings.json"));
            log::info!("adb: {} · llama-server: {}", adb_bin.display(), llama_bin.display());

            let state = Arc::new(state::AppState::new(adb::Adb::new(adb_bin), apk, node_version, llama_bin, config));
            app.manage(state.clone());
            updater::start(app.handle().clone(), state.clone());   // asks the user; never installs by itself
            updater::start_node_server(state.clone());             // node.apk for Wi-Fi nodes on :50055
            tauri::async_runtime::spawn(cluster::start(app.handle().clone(), state));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            cluster::get_snapshot,
            cluster::rescan,
            cluster::reconnect_node,
            cluster::update_usb_node,
            cluster::skip_node_update,
            updater::update_status,
            updater::check_updates,
            updater::set_update_check,
            updater::skip_update_version,
            updater::install_update,
            lan::wifi_node_update,
            llama::start_model,
            llama::stop_model,
            llama::restart_engine,
            llama::model_status,
            llama::set_keep_loaded,
            lan::add_lan_node,
            lan::add_discovered_node,
            lan::set_auto_add,
            lan::remove_lan_node,
            lan::test_lan_node,
            lan::set_node_weight,
            lan::set_node_enabled,
            lan::set_fast_start,
            agent::platform_info,
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
        .build(tauri::generate_context!())
        .expect("error while building tauri application");

    app.run(|handle, event| {
        if let tauri::RunEvent::Exit = event {
            // stop llama-server unless "keep model loaded" is on (no orphan engines holding :8080 / rpc slots)
            if let Some(st) = handle.try_state::<Arc<state::AppState>>() {
                let st = st.inner().clone();
                tauri::async_runtime::block_on(async move {
                    let _ = tokio::time::timeout(std::time::Duration::from_secs(6), llama::shutdown(&st)).await;
                });
            }
        }
    });
}
