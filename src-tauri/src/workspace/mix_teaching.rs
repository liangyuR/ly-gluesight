use super::*;
use crate::burst_judge::StationEvidenceState;
use crate::camera::WhiteBalance;
use sha2::{Digest, Sha256};
use std::collections::HashSet;

type Rect = [u32; 4];
type Line = [[f64; 2]; 2];

#[derive(Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum MixSource {
    Software,
    History {
        history_id: i64,
        seq: u64,
        segment: u32,
    },
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MixSampleInfo {
    token: String,
    size: [u32; 2],
    serial: String,
    white_balance: WhiteBalance,
    green_lines: Vec<Line>,
}

pub(super) struct Sample {
    info: MixSampleInfo,
    image: Arc<FrameImage>,
    software_triggered: bool,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MixRole {
    Yellow,
    Blue,
}

impl MixRole {
    fn id(self) -> &'static str {
        match self {
            Self::Yellow => "yellow",
            Self::Blue => "blue",
        }
    }
    fn label(self) -> u8 {
        match self {
            Self::Yellow => 2,
            Self::Blue => 3,
        }
    }
}

#[derive(Serialize, Deserialize, PartialEq)]
pub struct MixEndpoints {
    yellow: Option<String>,
    blue: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct Tile {
    width: u32,
    height: u32,
    rgb: Vec<u8>,
    labels: Vec<u8>,
}

#[derive(Serialize, Deserialize)]
struct Endpoint {
    info: MixSampleInfo,
    tile: Tile,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MixExample {
    history_id: i64,
    seq: u64,
    green_rects: Vec<Rect>,
}

fn dimensions(rect: Rect, width: u32, height: u32) -> Result<(u32, u32), String> {
    let [x, y, w, h] = rect;
    let right = x.checked_add(w).ok_or("选区越界")?;
    let bottom = y.checked_add(h).ok_or("选区越界")?;
    if w == 0 || h == 0 || right > width || bottom > height {
        return Err("选区为空或超出原图".into());
    }
    Ok((right, bottom))
}

fn paint(labels: &mut [u8], size: [u32; 2], rect: Rect, role: u8) -> Result<(), String> {
    let [x, y, ..] = rect;
    let (right, bottom) = dimensions(rect, size[0], size[1])?;
    for row in y..bottom {
        let offset = row as usize * size[0] as usize;
        let pixels = &mut labels[offset + x as usize..offset + right as usize];
        if pixels.iter().any(|v| *v != 0 && *v != role) {
            return Err("胶面取样区与背景重叠".into());
        }
        pixels.fill(role);
    }
    Ok(())
}

fn tile(image: &FrameImage, labels: &[u8]) -> Result<Tile, String> {
    if !image.is_color() || !image.consistent() || image.pixels.len() / 3 != labels.len() {
        return Err("取样帧或标签尺寸无效".into());
    }
    let (w, h) = (image.width, image.height);
    let (mut x0, mut y0, mut x1, mut y1) = (w, h, 0, 0);
    for (i, _) in labels.iter().enumerate().filter(|(_, l)| **l != 0) {
        let x = (i % w as usize) as u32;
        let y = (i / w as usize) as u32;
        x0 = x0.min(x);
        y0 = y0.min(y);
        x1 = x1.max(x);
        y1 = y1.max(y);
    }
    if x0 == w {
        return Err("没有选中有效的取样区域".into());
    }
    // 混合模型的饱和掩膜扩张 1 px，裁片保留这个真实邻域。
    x0 = x0.saturating_sub(1);
    y0 = y0.saturating_sub(1);
    x1 = x1.saturating_add(2).min(w);
    y1 = y1.saturating_add(2).min(h);
    let mut patch = Tile {
        width: x1 - x0,
        height: y1 - y0,
        rgb: Vec::new(),
        labels: Vec::new(),
    };
    for y in y0..y1 {
        let start = y as usize * w as usize + x0 as usize;
        let end = start + patch.width as usize;
        patch
            .rgb
            .extend_from_slice(&image.pixels[start * 3..end * 3]);
        patch.labels.extend_from_slice(&labels[start..end]);
    }
    Ok(patch)
}

fn history_sample(
    app: &AppHandle,
    camera: &str,
    id: i64,
    seq: u64,
    segment: u32,
) -> Result<Sample, String> {
    let store = app.state::<Store>();
    let summary = store.summary(id)?;
    let recipe = store.record_recipe(&summary)?.ok_or("历史件没有配方快照")?;
    let index = recipe
        .plan_cameras()
        .iter()
        .position(|c| c == camera)
        .ok_or("历史件没有这台相机")?;
    let detail = store.detail_frame(id, index, seq)?;
    let burst = detail.burst.as_ref().ok_or("历史件没有段配方帧")?;
    let frame = burst
        .frames
        .iter()
        .find(|f| f.camera == index && f.seq == seq)
        .ok_or("没有这台相机的帧")?;
    if frame
        .metrics
        .as_ref()
        .and_then(|m| m.corridor)
        .is_some_and(|s| s != segment)
    {
        return Err(format!("历史件本帧不属于所选第 {segment} 段"));
    }
    let cycle = detail.summary.cycle_id.as_deref().ok_or("缺少录制身份")?;
    let root = crate::project::records::record_root_of(
        &crate::storage::root(app),
        &detail.summary.project_id,
        detail.recording.directory.as_deref(),
    )?;
    let (image, serial, wb) =
        recorded::VerifiedFrames::new(&root, cycle)?.mix_sample(camera, frame)?;
    let green_lines = frame
        .metrics
        .as_ref()
        .and_then(|m| m.measurement_evidence.as_ref())
        .into_iter()
        .flat_map(|e| &e.stations)
        .filter(|s| s.state == StationEvidenceState::Exact)
        .filter_map(|s| {
            let (lo, hi) = (s.lo_edge?, s.hi_edge?);
            if lo.into_iter().chain(hi).any(|v| !v.is_finite()) {
                return None;
            }
            let inner = |f: f64| [lo[0] + (hi[0] - lo[0]) * f, lo[1] + (hi[1] - lo[1]) * f];
            Some([inner(0.25), inner(0.75)])
        })
        .collect();
    Ok(Sample {
        info: MixSampleInfo {
            token: String::new(),
            size: [image.width, image.height],
            serial,
            white_balance: wb,
            green_lines,
        },
        image: Arc::new(image),
        software_triggered: false,
    })
}

#[tauri::command]
pub async fn workspace_mix_sample(
    app: AppHandle,
    camera: String,
    source: MixSource,
) -> Result<MixSampleInfo, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let host = app.state::<WorkspaceHost>();
        let mut sample = match source {
            MixSource::Software => {
                let (image, serial, white_balance) = app
                    .state::<CycleHost>()
                    .camera
                    .capture_mix_reference(&camera)?;
                Sample {
                    info: MixSampleInfo {
                        token: String::new(),
                        size: [image.width, image.height],
                        serial,
                        white_balance,
                        green_lines: Vec::new(),
                    },
                    image,
                    software_triggered: true,
                }
            }
            MixSource::History {
                history_id,
                seq,
                segment,
            } => history_sample(&app, &camera, history_id, seq, segment)?,
        };
        sample.info.token = vision::unique_run_id(&format!("sample-{}", ly_plc::now_ms()));
        let info = sample.info.clone();
        host.mix_samples.lock().unwrap().insert(camera, sample);
        Ok(info)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub fn workspace_mix_sample_image(
    app: AppHandle,
    camera: String,
    token: String,
) -> Result<tauri::ipc::Response, String> {
    let host = app.state::<WorkspaceHost>();
    let samples = host.mix_samples.lock().unwrap();
    let sample = samples
        .get(&camera)
        .filter(|s| s.info.token == token)
        .ok_or("冻结样本已改变，请重新选择")?;
    Ok(preview_response(&sample.image))
}

fn endpoint_path(app: &AppHandle, camera: &str, role: MixRole) -> Result<PathBuf, String> {
    safe_id(camera)?;
    Ok(crate::storage::project_config(app)
        .join("mix-endpoints")
        .join(camera)
        .join(format!("{}.json", role.id())))
}

fn endpoint(app: &AppHandle, camera: &str, role: MixRole) -> Result<Option<Endpoint>, String> {
    crate::fsio::read_json(&endpoint_path(app, camera, role)?)
}

pub(super) fn publish_checks(w: &Workspace, configs: &[crate::camera::CameraConfig]) -> Vec<Check> {
    let mut result = Vec::new();
    for camera in &w.doc.inspect {
        for corridor in &camera.segments {
            let Some(mix) = &corridor.mix_check else {
                continue;
            };
            let ready = (|| -> Result<(), String> {
                let config = configs
                    .iter()
                    .find(|c| c.id == camera.camera)
                    .ok_or("未配置相机")?;
                if !config.image_mode.color() {
                    return Err("相机需要彩色模式".into());
                }
                if camera.mm_per_px.is_none() {
                    return Err("缺少相机像素当量".into());
                }
                let model = mix.model.as_ref().ok_or("缺少混胶参考模型")?;
                let serial = mix
                    .camera_serial
                    .as_deref()
                    .filter(|s| !s.is_empty())
                    .ok_or("模型缺少相机序列号")?;
                if mix.model_id.as_deref().is_none_or(|s| s.is_empty()) {
                    return Err("缺少模型版本".into());
                }
                let wb: WhiteBalance = serde_json::from_value(model["whiteBalance"].clone())
                    .map_err(|_| "模型缺少有效白平衡")?;
                wb.validate()?;
                if config.source == CameraSource::Mvs {
                    if model["datasetKind"] != "field" || serial != config.serial {
                        return Err("现场相机需要相同序列号的实拍模型".into());
                    }
                } else if config.source != CameraSource::Simulator
                    || model["datasetKind"] != "synthetic"
                {
                    return Err("合成联调模型需要独立模拟器相机".into());
                }
                let frame = w
                    .camera_teaching
                    .iter()
                    .find(|t| t.camera == camera.camera && t.segment == Some(corridor.segment))
                    .ok_or("缺少本段示教帧")?;
                if mix.background_rects.is_empty() {
                    return Err("缺少背景取样区".into());
                }
                for &rect in &mix.background_rects {
                    dimensions(rect, frame.frame.size[0], frame.frame.size[1])?;
                }
                Ok(())
            })();
            result.push(Check {
                advisory: false,
                name: format!("混胶色差 · {} 第 {} 段", camera.camera, corridor.segment),
                teaching_targets: Vec::new(),
                passed: ready.is_ok(),
                detail: ready
                    .err()
                    .unwrap_or_else(|| "参考模型、相机和取样范围已确认".into()),
            });
        }
    }
    result
}

fn endpoints(app: &AppHandle, camera: &str) -> Result<MixEndpoints, String> {
    Ok(MixEndpoints {
        yellow: endpoint(app, camera, MixRole::Yellow)?.map(|p| p.info.token),
        blue: endpoint(app, camera, MixRole::Blue)?.map(|p| p.info.token),
    })
}

#[tauri::command(async)]
pub fn workspace_mix_endpoints(app: AppHandle, camera: String) -> Result<MixEndpoints, String> {
    endpoints(&app, &camera)
}

#[tauri::command(async)]
pub fn workspace_mix_save_endpoint(
    app: AppHandle,
    camera: String,
    token: String,
    role: MixRole,
    rect: Rect,
) -> Result<MixEndpoints, String> {
    let host = app.state::<WorkspaceHost>();
    let samples = host.mix_samples.lock().unwrap();
    let sample = samples
        .get(&camera)
        .filter(|s| s.info.token == token)
        .ok_or("冻结样本已改变，请重新取样")?;
    if !sample.software_triggered {
        return Err("纯色端点需要使用 Software 拍摄的排胶样".into());
    }
    let mut labels = vec![0; sample.image.pixels.len() / 3];
    paint(&mut labels, sample.info.size, rect, role.label())?;
    let mut info = sample.info.clone();
    info.token = vision::unique_run_id(&format!("endpoint-{}", ly_plc::now_ms()));
    let selected = Endpoint {
        info,
        tile: tile(&sample.image, &labels)?,
    };
    let text = serde_json::to_string(&selected).map_err(|e| e.to_string())?;
    crate::fsio::write_atomic(&endpoint_path(&app, &camera, role)?, &text)?;
    endpoints(&app, &camera)
}

fn collect_green(sample: &Sample, rects: &[Rect], background: &[Rect]) -> Result<Tile, String> {
    let mut labels = vec![0; sample.image.pixels.len() / 3];
    if rects.is_empty() {
        for [a, b] in &sample.info.green_lines {
            let steps = (b[0] - a[0]).hypot(b[1] - a[1]).ceil() as usize;
            for i in 0..=steps {
                let t = i as f64 / steps.max(1) as f64;
                let x = (a[0] + t * (b[0] - a[0])).round();
                let y = (a[1] + t * (b[1] - a[1])).round();
                if x >= 0.
                    && y >= 0.
                    && x < sample.image.width as f64
                    && y < sample.image.height as f64
                {
                    labels[y as usize * sample.image.width as usize + x as usize] = 1;
                }
            }
        }
        if !labels.contains(&1) {
            return Err("本帧没有可取样的完整胶边，请手动框选合格胶面".into());
        }
    } else {
        for &rect in rects {
            paint(&mut labels, sample.info.size, rect, 1)?;
        }
    }
    for &rect in background {
        paint(&mut labels, sample.info.size, rect, 4)?;
    }
    tile(&sample.image, &labels)
}

fn fit_tiles(engine: &vision::Engine, tiles: &[Tile], wb: WhiteBalance) -> Result<Value, String> {
    let width = tiles.iter().map(|t| t.width).max().ok_or("缺少混胶样本")?;
    let height = tiles.iter().map(|t| t.height).max().unwrap();
    let atlas_height = height
        .checked_mul(u32::try_from(tiles.len()).map_err(|_| "取样帧数过大")?)
        .ok_or("取样图尺寸过大")?;
    let pixels = (width as usize)
        .checked_mul(atlas_height as usize)
        .ok_or("取样图尺寸过大")?;
    let mut rgb = Vec::new();
    let bytes = pixels.checked_mul(3).ok_or("取样图像过大")?;
    rgb.try_reserve_exact(bytes).map_err(|_| "样本图内存不足")?;
    rgb.resize(bytes, 0);
    let mut labels = vec![0; pixels];
    for (i, t) in tiles.iter().enumerate() {
        let area = t.width as usize * t.height as usize;
        if t.width == 0
            || t.height == 0
            || area.checked_mul(3) != Some(t.rgb.len())
            || area != t.labels.len()
        {
            return Err("保存的色端点样本尺寸无效，请重新取样".into());
        }
        for row in 0..t.height as usize {
            let dst = (i * height as usize + row) * width as usize;
            let len = t.width as usize;
            let src = row * len;
            rgb[dst * 3..(dst + len) * 3].copy_from_slice(&t.rgb[src * 3..(src + len) * 3]);
            labels[dst..dst + len].copy_from_slice(&t.labels[src..src + len]);
        }
    }
    let image = |node: &str, channels, pixels| lyflow_client::RunImageInput {
        node_id: node.into(),
        port: "image".into(),
        width,
        height: atlas_height,
        channels,
        depth: 1,
        pixels,
    };
    let result = engine.run_images(
        include_str!("../../graphs/mix_teach.lyflow.json"),
        "mix-fit",
        &[image("n_load", 3, rgb), image("n_labels", 1, labels)],
        &json!({"frameHeight":height,"whiteBalance":json!(wb).to_string()}),
    )?;
    if result.status() != "ok" {
        return Err(result.failure());
    }
    result
        .record("model")
        .cloned()
        .ok_or("混合模型算子没有输出模型".into())
}

fn save_model(
    app: &AppHandle,
    before: &Workspace,
    camera: &str,
    segment: u32,
    model: Value,
    serial: String,
    background: Vec<Rect>,
) -> Result<WorkspaceView, String> {
    let host = app.state::<WorkspaceHost>();
    let mut items = host.items.lock().unwrap();
    let mut w = items.get(&before.doc.id).ok_or("候选已删除")?.clone();
    w.expect(before.revision)?;
    if w.instance_id != before.instance_id {
        return Err("候选已重建，请重新拟合".into());
    }
    let mut doc = w.doc.clone();
    let check = doc
        .inspect
        .iter_mut()
        .find(|i| i.camera == camera)
        .and_then(|i| i.segments.iter_mut().find(|s| s.segment == segment))
        .and_then(|s| s.mix_check.as_mut())
        .ok_or("请先启用并保存本段混胶色差")?;
    let fingerprint =
        serde_json::to_vec(&(&serial, &model, &background)).map_err(|e| e.to_string())?;
    check.model_id = Some(format!("mix-{:x}", Sha256::digest(fingerprint)));
    check.camera_serial = Some(serial);
    check.background_rects = background;
    check.model = Some(model);
    update_doc(&mut w, doc)?;
    host.save(&w)?;
    items.insert(w.doc.id.clone(), w.clone());
    drop(items);
    view(app, w)
}

#[tauri::command(async)]
pub fn workspace_mix_fit(
    app: AppHandle,
    id: String,
    revision: u64,
    camera: String,
    segment: u32,
    examples: Vec<MixExample>,
    background_rects: Vec<Rect>,
    endpoints: MixEndpoints,
) -> Result<WorkspaceView, String> {
    let host = app.state::<WorkspaceHost>();
    let before = host.comparison_snapshot(&id, revision, None)?;
    if examples.is_empty() || background_rects.is_empty() {
        return Err("请选择合格历史帧和两侧背景取样区".into());
    }
    let yellow = endpoint(&app, &camera, MixRole::Yellow)?.ok_or("未保存纯黄端点")?;
    let blue = endpoint(&app, &camera, MixRole::Blue)?.ok_or("未保存纯蓝端点")?;
    if endpoints.yellow.as_ref() != Some(&yellow.info.token)
        || endpoints.blue.as_ref() != Some(&blue.info.token)
    {
        return Err("色端点已更新，请刷新后重新拟合".into());
    }
    let signature = &yellow.info;
    let matches = |s: &MixSampleInfo| {
        s.serial == signature.serial
            && s.white_balance == signature.white_balance
            && s.size == signature.size
    };
    if !matches(&blue.info) {
        return Err("纯黄、纯蓝样本相机、白平衡或尺寸不一致".into());
    }
    let mut tiles = vec![yellow.tile, blue.tile];
    let mut seen = HashSet::new();
    for source in &examples {
        if !seen.insert((source.history_id, source.seq)) {
            return Err("合格样本中包含重复帧".into());
        }
        let sample = history_sample(&app, &camera, source.history_id, source.seq, segment)?;
        if !matches(&sample.info) {
            return Err(format!(
                "历史件 {} 帧 {} 与色端点的相机、尺寸或白平衡不一致",
                source.history_id, source.seq
            ));
        }
        tiles.push(collect_green(
            &sample,
            &source.green_rects,
            &background_rects,
        )?);
    }
    let mut model = fit_tiles(
        burst::lyflow(&app)?.as_ref(),
        &tiles,
        signature.white_balance,
    )?;
    model["datasetKind"] = json!("field");
    {
        let _lock = host.mix_samples.lock().unwrap();
        if self::endpoints(&app, &camera)? != endpoints {
            return Err("拟合期间纯色端点已改变，请重新拟合".into());
        }
        save_model(
            &app,
            &before,
            &camera,
            segment,
            model,
            signature.serial.clone(),
            background_rects,
        )
    }
}

#[tauri::command(async)]
pub fn workspace_mix_import(
    app: AppHandle,
    id: String,
    revision: u64,
    camera: String,
    segment: u32,
    reference: Value,
    background_rects: Vec<Rect>,
) -> Result<WorkspaceView, String> {
    let before = app
        .state::<WorkspaceHost>()
        .comparison_snapshot(&id, revision, None)?;
    if reference["datasetKind"] != "synthetic"
        || reference["cameraId"] != camera
        || background_rects.is_empty()
    {
        return Err("请选择对应相机的 synthetic 联调模型与背景取样区".into());
    }
    let serial = reference["serial"]
        .as_str()
        .filter(|v| !v.is_empty())
        .ok_or("合成模型缺相机序列号")?;
    let wb: WhiteBalance = serde_json::from_value(reference["whiteBalance"].clone())
        .map_err(|_| "合成模型缺白平衡")?;
    wb.validate()?;
    let mut model = reference["model"].clone();
    if !model.is_object() {
        return Err("缺合成混合轴模型".into());
    }
    model["whiteBalance"] = json!(wb);
    model["datasetKind"] = json!("synthetic");
    let probe = json!({"schemaVersion":1,"id":"mix-import","nodes":[{"id":"n_load","op":"io.load_image","params":{"source":"inputs"}},{"id":"map","op":"glue.mix_map","params":{"model":model.to_string()}}],"edges":[{"id":"frame","from":{"node":"n_load","port":"image"},"to":{"node":"map","port":"image"}}],"outputs":{"check":{"node":"map","port":"maps.background"}}});
    let run = burst::lyflow(&app)?.run(
        &probe.to_string(),
        "mix-import",
        "",
        &FrameImage::rgb(1, 1, vec![60, 128, 40]),
        &json!({}),
    )?;
    if run.status() != "ok" {
        return Err(format!("原生算子拒绝此模型：{}", run.failure()));
    }
    for &r in &background_rects {
        dimensions(r, u32::MAX, u32::MAX)?;
    }
    save_model(
        &app,
        &before,
        &camera,
        segment,
        model,
        serial.into(),
        background_rects,
    )
}
