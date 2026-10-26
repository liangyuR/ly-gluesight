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

/// 有界等待配置门：布防、整圈采集开始或离线切换正在进行时排队等它释放，而不是立即拒绝。
/// 只能在阻塞线程（spawn_blocking 或普通线程）里调用，不能在 async 任务里调用。
pub(crate) fn lock_config_gate<'a>(
    gate: &'a tokio::sync::Mutex<()>,
    within: std::time::Duration,
    what: &str,
) -> Result<tokio::sync::MutexGuard<'a, ()>, String> {
    tauri::async_runtime::block_on(async { tokio::time::timeout(within, gate.lock()).await })
        .map_err(|_| format!("布防或其他配置操作 3 秒内仍未结束，暂不能{what}；请确认在线检测或离线产线已停止后重试"))
}

/// 等节拍释放上一次握手（busy 清除）后再取配置门；整段最多 3 秒。
pub(crate) fn wait_idle_config_gate<'a>(
    cycle: &'a CycleHost,
    what: &str,
) -> Result<Option<tokio::sync::MutexGuard<'a, ()>>, String> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    loop {
        while cycle.busy() && std::time::Instant::now() < deadline { std::thread::sleep(std::time::Duration::from_millis(50)); }
        let left = deadline.saturating_duration_since(std::time::Instant::now());
        let gate = lock_config_gate(&cycle.config_gate, left, what)?;
        if !cycle.busy() { return Ok(Some(gate)); }
        drop(gate);
        if std::time::Instant::now() >= deadline { return Ok(None); }
    }
}

#[tauri::command]
pub async fn workspace_capture_sample(app: AppHandle, id: String, revision: u64, round_id: String, expected: Verdict, correspondence_confirmed: bool) -> Result<WorkspaceView, String> {
    if !correspondence_confirmed { return Err("请确认验证采集与示教的轨迹、触发顺序对应一致".into()); }
    tauri::async_runtime::spawn_blocking(move || super::burst_release::capture_sample(app, id, revision, round_id, expected)).await.map_err(|e| e.to_string())?
}
