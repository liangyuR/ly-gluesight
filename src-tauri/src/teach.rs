//! 示教：随动相机的胶嘴标定试测、飞拍拍照点的模板与测量点示教、帧录制目录列表。

use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::json;
use tauri::{AppHandle, Manager, State};

use crate::caliper;
use crate::camera::CameraSource;
use crate::cycle::CycleHost;
use crate::follow::FollowCalib;
use crate::recipe::{InspectMode, Polarity};
use crate::vision::{self, ShotAssets, VisionAssets};

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProbeRequest {
    pub cam: u8,
    #[serde(default)]
    pub image_id: Option<String>,
    /// 页面上正在编辑、还没保存的标定
    pub calib: FollowCalib,
    /// 胶条离开胶嘴的方向在图像里的角度（度，x 轴起顺时针）；为空时自动找
    pub direction_deg: Option<f32>,
    pub polarity: Polarity,
    pub bead_width: f32,
    pub search_mm: f32,
    pub near_mm: f32,
    pub far_mm: f32,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProbePoint {
    /// 离胶嘴的距离（mm）
    pub l: f32,
    pub offset: Option<f32>,
    pub width: Option<f32>,
    pub st: u8,
    pub px: [f32; 2],
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProbeResult {
    pub image_size: [u32; 2],
    /// 实际用的方向（自动找时是找到的方向）
    pub direction_deg: f32,
    pub points: Vec<ProbePoint>,
}

fn probe_line(img: &crate::frame::FrameImage, r: &ProbeRequest, direction_deg: f32) -> Vec<ProbePoint> {
    let (s, c) = direction_deg.to_radians().sin_cos();
    let (dir, nrm) = ([c, s], [-s, c]);
    let mmpp = r.calib.mm_per_px;
    // 试测不设灰度差门槛：看的就是能不能找到胶条
    let q = caliper::Search { search_mm: r.search_mm, bead_width: r.bead_width, polarity: r.polarity, min_contrast: 0.0 };
    let mut points = Vec::new();
    let mut l = r.near_mm;
    while l <= r.far_mm {
        let p = [r.calib.nozzle[0] + dir[0] * l / mmpp, r.calib.nozzle[1] + dir[1] * l / mmpp];
        points.push(match caliper::read_at(img, mmpp, p, nrm, &q) {
            caliper::Reading::Bead { offset_mm, width_mm, px, .. } => ProbePoint { l, offset: Some(offset_mm), width: Some(width_mm), st: 0, px },
            _ => ProbePoint { l, offset: None, width: None, st: 1, px: p },
        });
        l += 0.5;
    }
    points
}

/// 方向的好坏：测到的点数，偏移越小越好（胶条正好沿这个方向）。
fn direction_score(points: &[ProbePoint]) -> f32 {
    points.iter().filter_map(|p| p.offset).map(|o| 1.0 / (1.0 + o.abs())).sum()
}

/// 随动标定试测：假设胶嘴身后是一条沿 direction 的直胶条，在最近一帧上逐毫米跑卡尺，
/// 看胶嘴位置、像素当量、搜索宽度与极性设得对不对。
#[tauri::command]
pub async fn teach_follow_probe(app: AppHandle, request: ProbeRequest) -> Result<ProbeResult, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let r = request;
        if app.state::<CycleHost>().busy() { return Err("工件正在检测，结束后再试测标定样本".into()); }
        r.calib.validate()?;
        if ![r.bead_width,r.search_mm,r.near_mm,r.far_mm].iter().all(|v|v.is_finite())||r.bead_width<=0.0||r.search_mm<=0.0||r.near_mm<0.0||r.far_mm<=r.near_mm||r.direction_deg.is_some_and(|d|!d.is_finite()) {
            return Err("胶宽、搜索范围或试测方向无效".into());
        }
        let slot = app.state::<CycleHost>().camera.slot(r.cam as usize).ok_or("相机不存在")?;
        // 海康相机空闲时不拷整帧，现取一张（就是画面上正在看的）；回放相机用"下一张"取到的那张
        let img = if let Some(id)=r.image_id.as_deref() { crate::workspace::station_image_ref(&app,r.cam,id)? } else { match slot.config().source {
            CameraSource::Mvs => slot.grab_full(Duration::from_millis(1500)).ok_or("1.5 s 内没收到这台相机的新帧：检查相机是否在出图")?,
            _ => slot.last_full().ok_or("这台相机还没有整帧图像：回放相机先按“下一张”取一帧")?,
        }};
        if img.width != r.calib.image_size[0] || img.height != r.calib.image_size[1] {
            return Err(format!("图像是 {}×{}，标定里写的是 {}×{}", img.width, img.height, r.calib.image_size[0], r.calib.image_size[1]));
        }
        let direction_deg = match r.direction_deg {
            Some(d) => d,
            // 每 3° 试一个方向，取胶条最贴合的那个，再在 ±3° 内按 0.5° 细找
            None => {
                let best = |range: Vec<f32>| {
                    range.into_iter().map(|d| (d, direction_score(&probe_line(&img, &r, d)))).max_by(|a, b| a.1.total_cmp(&b.1)).map_or(0.0, |x| x.0)
                };
                let coarse = best((0..120).map(|i| i as f32 * 3.0).collect());
                best((-6..=6).map(|i| coarse + i as f32 * 0.5).collect())
            }
        };
        let points = probe_line(&img, &r, direction_deg);
        if let Some(id)=r.image_id.as_deref() { crate::workspace::station_image_ref(&app,r.cam,id)?; }
        Ok(ProbeResult { image_size: [img.width, img.height], direction_deg, points })
    })
    .await
    .map_err(|e| e.to_string())?
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TeachStatus {
    pub dir: String,
    /// 各拍照点是否已示教（且与当前胶路几何一致）
    pub taught: Vec<bool>,
    pub stale: bool,
    /// 工位标定给出的像素当量
    pub mm_per_px: Option<f64>,
}

#[tauri::command]
pub fn teach_flyshot_status(app: AppHandle, cycle: State<'_, CycleHost>, recipe_id: String) -> Result<TeachStatus, String> {
    let recipe = cycle.recipe(&recipe_id).ok_or("配方不存在")?;
    let dir = vision::taught_dir(&app, &recipe.id)?;
    let assets = VisionAssets::load(&dir.join(vision::ASSETS_FILE));
    let stale = assets.as_ref().is_some_and(|a| a.fits(&recipe).is_err());
    let taught = (0..recipe.shot_count())
        .map(|k| !stale && assets.as_ref().and_then(|a| a.shots.get(k)).is_some_and(|s| !s.template.as_os_str().is_empty() && s.template.exists()))
        .collect();
    let mm_per_px = vision::calib_info(&vision::station_calib_path(&app, &recipe.camera)?).and_then(|c| c.mm_per_px);
    Ok(TeachStatus { dir: dir.display().to_string(), taught, stale, mm_per_px })
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShotTeach {
    pub recipe_id: String,
    pub k: usize,
    /// 模板在原图里的矩形 x, y, w, h（像素）
    pub rect: [u32; 4],
    /// 名义测量点叠加到图上时的对齐：平移（像素）与旋转（度）
    pub dx: f32,
    pub dy: f32,
    pub deg: f32,
    pub mm_per_px: f32,
}

/// 飞拍示教一个拍照点：从最近一帧裁模板，名义测量点按对齐参数换到像素坐标写成测量点文件。
#[tauri::command]
pub fn teach_flyshot_save(app: AppHandle, cycle: State<'_, CycleHost>, teach: ShotTeach) -> Result<TeachStatus, String> {
    let recipe = cycle.recipe(&teach.recipe_id).ok_or("配方不存在")?;
    if recipe.mode != InspectMode::FlyShot || teach.k >= recipe.shot_count() {
        return Err("不是飞拍配方，或拍照点序号超出范围".into());
    }
    if !(teach.mm_per_px > 0.0) {
        return Err("像素当量需为正".into());
    }
    let cam = cycle.camera.require(&recipe.camera)?;
    let img = cycle.camera.last_full(cam).ok_or("还没有整帧图像：先取一帧")?;
    let [x, y, w, h] = teach.rect;
    if w < 16 || h < 16 || x + w > img.width || y + h > img.height {
        return Err("模板矩形太小或超出图像".into());
    }
    let dir = vision::taught_dir(&app, &recipe.id)?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let template = dir.join(format!("k{}.template.pgm", teach.k));
    let mut crop = Vec::with_capacity((w * h) as usize);
    for row in y..y + h {
        let start = (row * img.width + x) as usize;
        crop.extend_from_slice(&img.pixels[start..start + w as usize]);
    }
    crate::replay::save_pgm(&template, &crate::frame::FrameImage::new(w, h, crop))?;

    let [cx, cy] = recipe.shots[teach.k];
    let (s, c) = teach.deg.to_radians().sin_cos();
    let (icx, icy) = (img.width as f32 / 2.0 + teach.dx, img.height as f32 / 2.0 + teach.dy);
    // 卡尺从内边往外找胶条：法向要朝胶路外侧，折线胶路的绕向不一定和圆角矩形一样
    let sign = recipe.outward_sign();
    let (mut points, mut normals, mut ids) = (Vec::new(), Vec::new(), Vec::new());
    for j in recipe.owned_points(teach.k) {
        let (u, v) = ((recipe.points.x[j] - cx) / teach.mm_per_px, (recipe.points.y[j] - cy) / teach.mm_per_px);
        let n = recipe.normal(j as f32 * recipe.spacing).map(|x| x * sign);
        points.push(json!([icx + u * c - v * s, icy + u * s + v * c]));
        normals.push(json!([n[0] * c - n[1] * s, n[0] * s + n[1] * c]));
        ids.push(j);
    }
    let stations = vision::save_stations(&dir, teach.k, points, normals, ids)?;

    let file = dir.join(vision::ASSETS_FILE);
    let mut assets = VisionAssets::load(&file).filter(|a| a.fits(&recipe).is_ok()).unwrap_or(VisionAssets {
        recipe_id: recipe.id.clone(),
        recipe_hash: recipe.geometry_hash(),
        sim_mm_per_px: None,
        calib: vision::station_calib_path(&app, &recipe.camera)?,
        shots: Vec::new(),
        camera: String::new(),
    });
    assets.camera = recipe.camera.clone();
    let empty = ShotAssets { template: PathBuf::new(), anchor: [0.0, 0.0], stations: PathBuf::new() };
    assets.shots.resize(recipe.shot_count(), empty);
    assets.shots[teach.k] = ShotAssets { template, anchor: [x as f64, y as f64], stations };
    assets.save(&file)?;
    app.state::<vision::VisionHost>().forget(&recipe.id);
    teach_flyshot_status(app.clone(), cycle, recipe.id.clone())
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordEntry {
    pub path: String,
    pub name: String,
    pub frames: usize,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordListing {
    pub root: String,
    pub items: Vec<RecordEntry>,
}

/// 帧录制目录，新的在前，最多 200 条。
#[tauri::command]
pub fn records_list(cycle: State<'_, CycleHost>) -> RecordListing {
    let root = cycle.recorder.root().to_path_buf();
    let mut items = Vec::new();
    if let Ok(days) = std::fs::read_dir(&root) {
        for day in days.flatten().map(|d| d.path()).filter(|p| p.is_dir() && !p.ends_with("_pending")) {
            for part in std::fs::read_dir(&day).into_iter().flatten().flatten().map(|e| e.path()).filter(|p| p.is_dir()) {
                let frames = std::fs::read_dir(&part).map(|rd| rd.flatten().filter(|e| e.path().extension().is_some_and(|x| x == "pgm")).count()).unwrap_or(0);
                let name = part.file_name().and_then(|n| n.to_str()).unwrap_or_default().to_string();
                items.push(RecordEntry { path: part.display().to_string(), name, frames });
            }
        }
    }
    items.sort_by(|a, b| b.name.cmp(&a.name));
    items.truncate(200);
    RecordListing { root: root.display().to_string(), items }
}
