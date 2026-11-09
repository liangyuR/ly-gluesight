use super::*;

#[derive(Serialize)]
pub struct CenterlineResult {
    pub path: Vec<[f32; 2]>,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failure: Option<super::centerline::ExtractionFailure>,
}

#[tauri::command]
pub fn workspace_retry_publish(app: AppHandle, id: String, revision: u64) -> Result<WorkspaceView, String> {
    let host = app.state::<WorkspaceHost>(); let mut items = host.items.lock().unwrap();
    let mut w = items.get(&id).ok_or("候选不存在")?.clone();
    w.retry_publish(revision)?;
    host.save(&w)?; items.insert(id, w.clone());
    let _ = app.state::<CycleHost>().tx.send(cycle::Input::Refresh);
    view(&app, w)
}

#[tauri::command]
pub fn workspace_abandon_publish(app: AppHandle, id: String, revision: u64) -> Result<WorkspaceView, String> {
    let host = app.state::<WorkspaceHost>(); let mut items = host.items.lock().unwrap();
    let mut w = items.get(&id).ok_or("候选不存在")?.clone();
    w.abandon_publish(revision)?;
    host.save(&w)?; items.insert(id, w.clone());
    view(&app, w)
}
