//! 示教：飞拍拍照点的模板与测量点示教、帧录制目录列表。

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::json;
use tauri::{AppHandle, Manager, State};

use crate::cycle::CycleHost;
use crate::vision::{self, ShotAssets, VisionAssets};

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TeachStatus {
    pub dir: String,
    /// 各拍照点是否已示教（且与当前胶路几何一致）
    pub taught: Vec<bool>,
    pub stale: bool,
    /// 各拍照点所用工位标定给出的像素当量
    pub mm_per_px: Vec<Option<f64>>,
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
    let mm_per_px = (0..recipe.shot_count())
        .map(|k| Ok(vision::calib_info(&vision::shot_calib_path(&app, &recipe, k)?).and_then(|c| c.mm_per_px)))
        .collect::<Result<_, String>>()?;
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
    if teach.k >= recipe.shot_count() {
        return Err("拍照点序号超出范围".into());
    }
    if !(teach.mm_per_px > 0.0) {
        return Err("像素当量需为正".into());
    }
    let shot = &recipe.shots[teach.k];
    let cam = cycle.camera.require(&shot.camera)?;
    let img = cycle.camera.last_full(cam).ok_or_else(|| format!("相机 {} 还没有整帧图像：先取一帧", shot.camera))?;
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

    let [cx, cy] = shot.center;
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
        shots: Vec::new(),
    });
    let empty = ShotAssets { template: PathBuf::new(), anchor: [0.0, 0.0], stations: PathBuf::new(), calib: PathBuf::new() };
    assets.shots.resize(recipe.shot_count(), empty);
    let calib = vision::shot_calib_path(&app, &recipe, teach.k)?;
    assets.shots[teach.k] = ShotAssets { template, anchor: [x as f64, y as f64], stations, calib };
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
