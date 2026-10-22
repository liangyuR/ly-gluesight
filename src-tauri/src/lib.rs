mod storage_startup;
mod candidate;
mod calibration;
mod applog;
mod arming;
mod audit;
mod burst_judge;
mod camera;
mod commands;
mod cycle;
mod cycle_ids;
mod diagnostics;
mod disk_guard;
mod frame;
mod frame_group;
mod fsio;
mod history;
mod handshake;
mod handshake_audit;
mod inspection;
mod instance;
mod judge;
mod locate_health;
mod measure;
mod mvs;
mod offline;
mod packaging;
mod plan;
mod plc;
mod plc_plan;
mod plc_session;
mod production;
mod recipe;
mod recipe_capture;
mod recipe_api;
mod release;
mod recorder;
mod recording_health;
mod image_file;
mod settings;
mod site;
mod shot_router;
mod segmentation;
mod simimage;
mod store;
mod storage;
mod managed_samples;
mod teach;
mod timeline;
mod vision;
mod workspace;
mod ziparchive;

use tauri::Manager;

pub use packaging::check_packaged_runtime;

fn open_store(app: &tauri::AppHandle) -> Result<store::Store, String> {
    store::Store::open_deferred_recovery(&storage::root(app).join("inspection.db"))
}

pub fn run() {
    let mut context = tauri::generate_context!();
    for window in &mut context.config_mut().app.windows { window.create = false; }
    tauri::Builder::default()
        .setup(|app| {
            app.manage(storage_startup::Startup::default());
            let mut prepared = storage::selected_root().and_then(|root| {
                storage::prepare(app.handle(), &root)?;
                Ok(root)
            });
            if let Ok(root) = &prepared {
                if storage::initialized(root) {
                    let loaded = storage::initialize(app.handle(), root.clone())
                        .and_then(|_| start_services(app.handle(), root.clone()).map_err(|error| error.to_string()));
                    match loaded {
                        Ok(()) => return Ok(()),
                        Err(error) => prepared = Err(error),
                    }
                }
            }
            if let Err(error) = &prepared {
                eprintln!("{error}");
                if headless(app.handle()) { std::process::exit(1); }
            }
            let profile = match &prepared {
                Ok(root) => storage::webview(&storage::bootstrap_root(root))?,
                Err(_) => std::env::temp_dir().join(format!("GlueSight-startup-error-{}", std::process::id())),
            };
            tauri::WebviewWindowBuilder::new(app, "storage-startup", tauri::WebviewUrl::App("storage-startup.html".into()))
                .title("GlueSight · 资料准备").inner_size(720.0, 480.0).center().resizable(false)
                .visible(!headless(app.handle()))
                .data_directory(profile).build()?;
            if let Err(error) = prepared {
                storage_startup::failed(app.handle(), error);
                return Ok(());
            }
            let app = app.handle().clone();
            std::thread::spawn(move || {
                let initialized = storage::selected_root().and_then(|root| {
                    storage::initialize(&app, root.clone())?;
                    Ok(root)
                });
                match initialized {
                    Ok(root) => {
                        let ready_app = app.clone();
                        if let Err(error) = app.run_on_main_thread(move || {
                            if let Err(error) = start_services(&ready_app, root) {
                                startup_failed(ready_app, error.to_string());
                            }
                        }) {
                            startup_failed(app, error.to_string());
                        }
                    }
                    Err(error) => startup_failed(app, error),
                }
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            storage_startup::storage_startup_status,
            storage_startup::storage_startup_answer,
            storage::storage_status,
            storage::storage_open_directory,
            storage::storage_preferences_get,
            storage::storage_preferences_update,
            commands::app_info,
            commands::app_open_log_dir,
            applog::applog_query,
            applog::applog_usage,
            diagnostics::diagnostics_export,
            timeline::history_timeline,
            timeline::history_find_cycle,
            commands::engine_status,
            plc::plc_get_config,
            plc::plc_save_config,
            plc::plc_connect,
            plc::plc_disconnect,
            plc::plc_get_status,
            plc::plc_get_values,
            plc::plc_write_point,
            plc::plc_query_logs,
            plc::plc_point_history,
            plc::plc_check_address,
            plc::plc_s7_phase1_template,
            cycle::cycle_snapshot,
            cycle::cycle_logs,
            cycle::cycle_part_data,
            cycle::cycle_recipes,
            cycle::cycle_layout,
            cycle::cycle_get_settings,
            cycle::cycle_save_settings,
            cycle::cycle_select_recipe,
            cycle::cycle_reset,
            cycle::cycle_readiness,
            camera::camera_rig_status,
            camera::camera_rig_config,
            camera::camera_bandwidth,
            camera::camera_save_config,
            camera::camera_add,
            camera::camera_remove,
            camera::camera_list_devices,
            camera::camera_preview,
            camera::camera_soft_trigger,
            camera::camera_set_imaging,
            camera::camera_release,
            camera::camera_reconnect,
            recipe_api::recipe_list,
            recipe_api::recipe_doc,
            recipe_api::recipe_preview,
            recipe_api::recipe_template,
            recipe_api::recipe_delete,
            recipe_api::recipe_delete_unavailable,
            teach::records_list,
            history::history_query,
            history::history_detail,
            history::history_neighbors,
            history::history_recipe,
            history::history_rejudge,
            history::history_export,
            history::reveal_path,
            vision::vision_calib_info,
            vision::vision_calibrate,
            site::site_status,
            site::site_enter_offline,
            site::site_leave_offline,
            site::site_preview_sample_set,
            site::site_pick_dir,
            offline::offline_run,
            offline::offline_capture,
            offline::offline_stop,
            offline::offline_inject,
            workspace::workspace_list,
            offline::offline_pace,
            offline::offline_fault_reset,
            recipe_capture::recipe_capture_start,
            recipe_capture::recipe_capture_get,
            recipe_capture::recipe_capture_stop,
            recipe_capture::recipe_capture_list,
            recipe_capture::recipe_capture_image,
            workspace::workspace_set_views,
            workspace::workspace_select_group_image,
            workspace::workspace_save_draft,
            workspace::workspace_progress,
            workspace::workspace_check_calibration,
            workspace::workspace_extract_centerline,
            workspace::workspace_debug_run,
            workspace::workspace_retry_publish,
            workspace::workspace_abandon_publish,
            workspace::workspace_adopt_capture,
            workspace::workspace_capture_sample,
            workspace::workspace_get,
            workspace::workspace_create,
            workspace::workspace_delete,
            workspace::workspace_save_doc,
            workspace::workspace_set_camera_imaging,
            workspace::workspace_select_view,
            workspace::workspace_image,
            workspace::workspace_trial,
            workspace::workspace_save_params,
            workspace::workspace_save_teach,
            workspace::workspace_fixture_check,
            workspace::workspace_fixture_trial,
            workspace::workspace_apply_fixture_suggestion,
            workspace::workspace_restore_teach,
            workspace::workspace_save_overview,
            workspace::workspace_validate,
            workspace::workspace_import_sample,
            workspace::workspace_publish,
            workspace::workspace_record_images,
            workspace::workspace_record_image,
            workspace::workspace_history_capture,
            workspace::workspace_compare,
            workspace::workspace_compare_original,
            workspace::workspace_comparisons,
            workspace::workspace_runtime_overview,
            workspace::workspace_live_image,
            workspace::workspace_station_capture,
            workspace::workspace_station_import,
            workspace::workspace_station_image,
        ])
        .run(context)
        .expect("error while running tauri application");
}

fn headless(app: &tauri::AppHandle) -> bool {
    !app.config().app.windows.iter().any(|window| window.visible)
}

fn startup_failed(app: tauri::AppHandle, error: String) {
    eprintln!("{error}");
    if headless(&app) {
        app.exit(1);
        return;
    }
    storage_startup::failed(&app, error);
}

fn start_services(app: &tauri::AppHandle, root: std::path::PathBuf) -> Result<(), Box<dyn std::error::Error>> {
    if let Err(error) = applog::init(app) { eprintln!("{error}"); }
    app.manage(open_store(app)?);
    app.manage(vision::VisionHost::default());
    app.manage(production::ProductionHost::default());
    app.manage(offline::OfflineHost::default());
    let site_note = site::restore_choice(app);
    app.manage(cycle::CycleHost::init(app)?);
    if let Some(note) = site_note { cycle::log(app, "warn", "站点模式", note); }
    app.manage(workspace::WorkspaceHost::init(app)?);
    app.manage(plc::PlcHost::init(app)?);
    for config in &app.config().app.windows {
        tauri::WebviewWindowBuilder::from_config(app, config)?.data_directory(storage::webview(&root)?).build()?;
    }
    if let Some(window) = app.get_webview_window("storage-startup") { window.close()?; }
    if !storage::migrated_this_launch(app) { plc::PlcHost::start_if_configured(app); }
    cycle::CycleHost::start(app);
    camera::CameraRig::start(app, !storage::migrated_this_launch(app));
    measure::apply_settings(app);
    let handle = app.clone();
    tauri::async_runtime::spawn_blocking(move || cycle::purge_history(&handle));
    Ok(())
}
