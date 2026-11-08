use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Camera {
    pub id: String,
    pub port: u16,
    pub frames: Vec<PathBuf>,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Config {
    pub plc_port: u16,
    pub plc_db: u16,
    pub pc_db: u16,
    pub product_code: u16,
    pub sn_prefix: String,
    pub frame_rate: f64,
    pub capture_lead_ms: u64,
    pub interval_ms: u64,
    pub result_timeout_ms: u64,
    pub pass_frames: Vec<usize>,
    pub pass_gap_ms: u64,
    pub cameras: Vec<Camera>,
}

impl Config {
    pub fn load(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
        let mut config: Self =
            serde_json::from_str(text.trim_start_matches('\u{feff}')).map_err(|e| e.to_string())?;
        let root = path.parent().unwrap_or(Path::new("."));
        for camera in &mut config.cameras {
            for frame in &mut camera.frames {
                if frame.is_relative() {
                    *frame = root.join(&frame);
                }
            }
        }
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.plc_port == 0 || self.pc_db == 0 || self.plc_db == 0 || self.pc_db == self.plc_db {
            return Err("PLC 端口和 DB 必须非零，两个 DB 必须不同".into());
        }
        if !(1..=32767).contains(&self.product_code)
            || self.sn_prefix.is_empty()
            || self.sn_prefix.len() > 28
            || !self.sn_prefix.bytes().all(|b| (0x21..=0x7e).contains(&b))
        {
            return Err("型号需在 1–32767，SN 前缀需为 1–28 个非空格 ASCII 可打印字符".into());
        }
        if !self.frame_rate.is_finite()
            || !(1.0..=60.0).contains(&self.frame_rate)
            || !(2000..=60000).contains(&self.capture_lead_ms)
            || !(200..=60000).contains(&self.interval_ms)
            || !(1000..=600000).contains(&self.result_timeout_ms)
            || !(100..=60000).contains(&self.pass_gap_ms)
        {
            return Err("帧率需 1–60 Hz，接件提前量 2000–60000 ms，件间隔 200–60000 ms，结果超时 1000–600000 ms，笔间隔 100–60000 ms".into());
        }
        let total = self
            .pass_frames
            .iter()
            .try_fold(0usize, |sum, n| sum.checked_add(*n))
            .ok_or("帧数溢出")?;
        if total == 0
            || total > 100000
            || self.pass_frames.contains(&0)
            || !(1..=8).contains(&self.cameras.len())
        {
            return Err("需 1–8 台相机，笔帧数非零，总帧数不超过 100000".into());
        }
        let mut ports = std::collections::HashSet::from([self.plc_port]);
        let mut ids = std::collections::HashSet::new();
        for camera in &self.cameras {
            if camera.id.is_empty()
                || !camera
                    .id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
                || !ids.insert(&camera.id)
                || camera.port == 0
                || !ports.insert(camera.port)
                || camera.frames.len() != total
            {
                return Err(format!(
                    "相机 {} 的 ID/端口重复或非法，或图像数不等于笔帧数之和 {total}",
                    camera.id
                ));
            }
            let mut dimensions = None;
            for path in &camera.frames {
                let (w, h) = image::image_dimensions(path)
                    .map_err(|e| format!("{}: {e}", path.display()))?;
                if w == 0 || h == 0 || (w as u64) * (h as u64) * 3 > 64 * 1024 * 1024 {
                    return Err(format!("{} 图像尺寸超限", path.display()));
                }
                if dimensions.is_some_and(|size| size != (w, h)) {
                    return Err(format!("相机 {} 的帧尺寸不一致", camera.id));
                }
                dimensions = Some((w, h));
                if path
                    .extension()
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("pgm"))
                {
                    use std::io::Read;
                    let mut header = [0u8; 256];
                    let len = std::fs::File::open(path)
                        .map_err(|e| e.to_string())?
                        .read(&mut header)
                        .map_err(|e| e.to_string())?;
                    if header[..len].windows(5).any(|w| w == b"bayer") {
                        return Err(format!(
                            "{} 是 Bayer 原图，请先用 Prepare-ProductionSimulation.py 转为 RGB PNG",
                            path.display()
                        ));
                    }
                }
            }
        }
        Ok(())
    }
}

pub fn init_demo(root: &Path) -> Result<(), String> {
    if root.exists() {
        return Err("演示目录已存在，请使用新目录，避免覆盖样本".into());
    }
    std::fs::create_dir_all(root).map_err(|e| e.to_string())?;
    let mut cameras = Vec::new();
    for n in 1..=3 {
        let image = image::GrayImage::from_fn(640, 480, |x, y| {
            image::Luma([if (300..340).contains(&x) && (20..460).contains(&y) {
                220
            } else {
                30
            }])
        });
        let filename = format!("cam{n}.png");
        image
            .save(root.join(&filename))
            .map_err(|e| e.to_string())?;
        cameras.push(Camera {
            id: format!("cam{n}"),
            port: 12000 + n,
            frames: vec![filename.into(); 30],
        });
    }
    let config = Config {
        plc_port: 1102,
        plc_db: 4,
        pc_db: 2,
        product_code: 1,
        sn_prefix: "SIM".into(),
        frame_rate: 10.0,
        capture_lead_ms: 2000,
        interval_ms: 2000,
        result_timeout_ms: 10000,
        pass_frames: vec![30],
        pass_gap_ms: 500,
        cameras,
    };
    std::fs::write(
        root.join("simulator.json"),
        serde_json::to_vec_pretty(&config).unwrap(),
    )
    .map_err(|e| e.to_string())
}
