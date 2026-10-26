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
mod frame_align;
mod frame_group;
mod fsio;
mod history;
mod handshake;
mod handshake_audit;
mod inspection;
mod instance;
mod judge;
mod measure;
mod mvs;
mod offline;
mod packaging;
mod plan;
mod plc;
mod plc_plan;
mod plc_session;
mod production;
mod project;
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
mod teaching;
mod timeline;
mod vision;
mod workspace;
mod ziparchive;

use tauri::Manager;

pub use packaging::check_packaged_runtime;

/// 吞吐探针（examples/throughput_probe.rs）与分段离线评估（examples/segment_eval.rs）所需的内部接口，只为 example 暴露；
/// 不是稳定 API，应用本身不用。
#[doc(hidden)]
pub mod probe_api {
    pub use crate::burst_judge::FrameState;
    pub use crate::calibration::calibrate_counts;
    pub use crate::frame_align::{align_cuts, downsample, Sequence, ALIGN_FACTOR};
    pub use crate::frame_group::FrameGroup;
    pub use crate::image_file::{load as load_image, save_pgm};
    pub use crate::plan::SegmentKind;
    pub use crate::production::{burst_graphs, Prepared, GRAPH_VERSION_BURST};
    pub use crate::recipe::{
        burst_template, DetectParams, Polarity, Recipe, RecipeDoc, SegmentCorridor,
    };
    pub use crate::release::{
        load as load_release, publish_burst, BurstPublishInput, CameraInput, CorridorInput,
        ResourceSource, ValidationSite, Versions,
    };
    pub use crate::segmentation::{Proportional, SegInput, SegOutcome, Segmenter};
    pub use crate::vision::{Engine, FrameImage};

    /// 发布包加载后的预热：在自己的示教原图上跑一遍（`Prepared::warm` 是 crate 内可见）。
    pub fn warm(prepared: &Prepared) -> Result<(), String> {
        prepared.warm()
    }
}

fn open_store(app: &tauri::AppHandle) -> Result<store::Store, String> {
    store::Store::open_deferred_recovery(&storage::project(app).join("inspection.db"))
}

fn open_project(app: &tauri::AppHandle, root: &std::path::Path) -> Result<(), String> {
    match project::resolve(root)? {
        project::Resolution::Open(active) => {
            app.manage(active);
            Ok(())
        }
        project::Resolution::NeedsChoice(reason) => {
            Err(format!("{}{reason}", storage_startup::CHOOSE_PROJECT))
        }
    }
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
                eprintln!(
                    "{}",
                    error.trim_start_matches(storage_startup::CHOOSE_PROJECT)
                );
                if headless(app.handle()) {
                    std::process::exit(1);
                }
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
                storage_startup::report(app.handle(), error);
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
            storage_startup::storage_startup_projects,
            storage_startup::storage_startup_open_project,
            storage_startup::storage_startup_create_project,
            storage::storage_status,
            storage::storage_open_directory,
            project::project_current,
            project::project_list,
            project::manage::project_usage,
            project::manage::project_create,
            project::manage::project_rename,
            project::manage::project_delete,
            project::project_switch,
            project::transfer::project_export,
            project::transfer::project_import_pick,
            project::transfer::project_import,
            project::transfer::project_copy,
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
            plc::plc_export_logs,
            plc::plc_point_history,
            plc::plc_check_address,
            plc::plc_s7_phase1_template,
            cycle::cycle_snapshot,
            cycle::cycle_logs,
            cycle::cycle_recipes,
            cycle::cycle_layout,
            cycle::cycle_get_settings,
            cycle::cycle_save_settings,
            cycle::cycle_set_maintenance,
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
            recipe_api::recipe_plan_export,
            teach::records_list,
            history::history_query,
            history::history_detail,
            history::history_neighbors,
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
            recipe_capture::recipe_capture_plc_status,
            recipe_capture::recipe_capture_list,
            recipe_capture::recipe_capture_frame_image,
            recipe_capture::recipe_capture_round_thumbs,
            workspace::workspace_debug_run,
            workspace::workspace_retry_publish,
            workspace::workspace_abandon_publish,
            workspace::workspace_capture_sample,
            workspace::workspace_get,
            workspace::workspace_create,
            workspace::workspace_delete,
            workspace::workspace_transfer_export,
            workspace::workspace_transfer_pick,
            workspace::workspace_transfer_import,
            workspace::workspace_save_doc,
            workspace::workspace_set_camera_imaging,
            workspace::workspace_image,
            workspace::workspace_validate,
            workspace::workspace_publish,
            workspace::workspace_collect_checks,
            workspace::workspace_publish_collect,
            workspace::workspace_compare,
            workspace::workspace_compare_original,
            workspace::workspace_comparisons,
            workspace::workspace_live_frame_image,
            workspace::workspace_station_capture,
            workspace::workspace_station_import,
            workspace::workspace_station_image,
            workspace::workspace_update_segments,
            workspace::workspace_teach_segment_corridor,
            workspace::workspace_clear_segment_corridor,
            workspace::workspace_extract_corridor,
            workspace::workspace_calibration_add_round,
            workspace::workspace_calibration_remove_round,
            workspace::workspace_calibration_round_features,
            workspace::workspace_calibration_frame,
            workspace::workspace_publish_checks,
            workspace::workspace_calibration_apply,
            workspace::workspace_capture_review_save,
            workspace::workspace_segment_template_save,
            workspace::workspace_capture_review_suggest,
            workspace::workspace_capture_review_confirm_cuts,
            workspace::workspace_capture_review_confirm,
            workspace::workspace_capture_review_set_template,
            workspace::workspace_capture_review_apply_template,
            workspace::workspace_capture_review_exclude,
            workspace::workspace_history_candidates,
            workspace::workspace_history_import,
            workspace::workspace_confirm_tolerance,
            workspace::workspace_record_frame_image,
        ])
        .run(context)
        .expect("error while running tauri application");
}

fn headless(app: &tauri::AppHandle) -> bool {
    !app.config().app.windows.iter().any(|window| window.visible)
}

fn startup_failed(app: tauri::AppHandle, error: String) {
    eprintln!(
        "{}",
        error.trim_start_matches(storage_startup::CHOOSE_PROJECT)
    );
    if headless(&app) {
        app.exit(1);
        return;
    }
    storage_startup::report(&app, error);
}

fn start_services(
    app: &tauri::AppHandle,
    root: std::path::PathBuf,
) -> Result<(), Box<dyn std::error::Error>> {
    if let Err(error) = applog::init(app) {
        eprintln!("{error}");
    }
    if let Err(error) = project::manage::purge_trash(&root) {
        applog::write("warn", "项目删除未完成", &error);
    }
    open_project(app, &root)?;
    let leftovers = project::legacy_leftovers(&root);
    if !leftovers.is_empty() {
        applog::write(
            "warn",
            "旧资料未读取",
            &format!(
                "资料目录根下仍有项目化之前的资料，软件不再读取，可人工清理：{}",
                leftovers.join("、")
            ),
        );
    }
    app.manage(open_store(app)?);
    app.manage(vision::VisionHost::default());
    app.manage(production::ProductionHost::default());
    app.manage(offline::OfflineHost::default());
    let site_note = site::restore_choice(app);
    app.manage(cycle::CycleHost::init(app)?);
    if let Some(note) = site_note { cycle::log(app, "warn", "站点模式", note); }
    app.manage(workspace::WorkspaceHost::init(app)?);
    app.manage(plc::PlcHost::init(app)?);
    commands::log_config(app);
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
