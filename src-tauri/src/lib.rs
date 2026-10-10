mod audit;
mod camera;
mod commands;
mod cycle;
mod cycle_ids;
mod frame;
mod fsio;
mod history;
mod handshake;
mod inspection;
mod instance;
mod judge;
mod measure;
mod mvs;
mod plc;
mod plc_plan;
mod plc_session;
mod production;
mod recipe;
mod recipe_api;
mod release;
mod recorder;
mod replay;
mod settings;
mod shot_router;
mod sim;
mod simimage;
mod store;
mod teach;
mod vision;
mod workspace;

use tauri::Manager;

fn open_store(app: &tauri::App) -> Result<store::Store, String> {
    store::Store::open(&app.path().app_data_dir().map_err(|e| e.to_string())?.join("inspection.db"))
}

pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            app.manage(instance::acquire(&app.path().app_data_dir().map_err(|error| error.to_string())?)?);
            app.manage(open_store(app)?);
            app.manage(vision::VisionHost::default());
            app.manage(production::ProductionHost::default());
            app.manage(cycle::CycleHost::init(app.handle())?);
            app.manage(workspace::WorkspaceHost::init(app.handle())?);
            app.manage(plc::PlcHost::init(app.handle())?);
            plc::PlcHost::start_if_configured(app.handle());
            cycle::CycleHost::start(app.handle());
            camera::CameraRig::start(app.handle());
            measure::apply_settings(app.handle());
            let handle = app.handle().clone();
            tauri::async_runtime::spawn_blocking(move || cycle::purge_history(&handle));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::app_info,
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
            plc::plc_recipe_plan,
            cycle::cycle_snapshot,
            cycle::cycle_logs,
            cycle::cycle_part_data,
            cycle::cycle_recipes,
            cycle::cycle_layout,
            cycle::cycle_get_settings,
            cycle::cycle_save_settings,
            cycle::cycle_select_recipe,
            cycle::cycle_reset,
            camera::camera_rig_status,
            camera::camera_rig_config,
            camera::camera_save_config,
            camera::camera_add,
            camera::camera_remove,
            camera::camera_list_devices,
            camera::camera_pick_replay_dir,
            camera::camera_preview,
            camera::camera_soft_trigger,
            camera::camera_dry_run_start,
            camera::camera_dry_run_get,
            camera::camera_dry_run_stop,
            recipe_api::recipe_list,
            recipe_api::recipe_doc,
            recipe_api::recipe_preview,
            recipe_api::recipe_template,
            recipe_api::recipe_save,
            recipe_api::recipe_delete,
            teach::records_list,
            history::history_query,
            history::history_detail,
            history::history_recipe,
            history::history_rejudge,
            history::history_export,
            history::reveal_path,
            vision::vision_calib_info,
            vision::vision_calibrate,
            sim::sim_status,
            sim::sim_start,
            sim::sim_stop,
            sim::sim_robot_trigger,
            workspace::workspace_list,
            workspace::workspace_get,
            workspace::workspace_create,
            workspace::workspace_delete,
            workspace::workspace_save_doc,
            workspace::workspace_capture,
            workspace::workspace_select_view,
            workspace::workspace_import_image,
            workspace::workspace_image,
            workspace::workspace_trial,
            workspace::workspace_save_params,
            workspace::workspace_save_teach,
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
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
