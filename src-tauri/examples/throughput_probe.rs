//! P1 吞吐摸底探针（docs/decisions/segment-burst.md 第七节）。不是测试，没有要防的回归，所以不放在 #[test] 里。
//!
//! 量 1280×1024 单帧在发布加载路径上的测量耗时（p50/p95/max，串行与并发）和 PGM 连续写盘速率，
//! 数值回填 MEASURE_WORKERS 与 IMAGE_BUDGET_PER_CAMERA。只断言量的是正常路径（胶测得到、文件写满），数值本身不设门槛。
//! 只用中线测量的算子（glue.taught_path + glue.bead_width），没有特征定位，与段模型走廊测量是同一组算子；
//! 不要求带 lyFlow#32 的构建，已发布的运行库即可。
//!
//! 运行（写约 4 GB 临时文件，结束后自动删除，约 1 分钟；用 dev profile，与 7.1 节记录的数值同一构建配置）：
//!
//! ```powershell
//! $env:LYFLOW_CORE_DLL = 'src-tauri\target\dependency-cache\lyflow\1.1.5\lyflow_core.dll'
//! cargo run --manifest-path src-tauri/Cargo.toml --example throughput_probe
//! ```
//!
//! 现场机上把 LYFLOW_CORE_DLL 指向安装目录里的 lyflow_core.dll 即可；没有设置时程序打印这段说明后退出。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Instant;

use gluesight_lib::probe_api::{
    graphs, load_release, publish_release, samples, save_pgm, warm, DetectParams, Engine, FrameImage, JudgeParams, Polarity, Prepared,
    PublishInput, RecipeDoc, ResourceSource, ShotInput, ShotLimits, ValidationSite, Versions, GRAPH_VERSION,
};
use serde_json::json;

const PROBE_W: u32 = 1280;
const PROBE_H: u32 = 1024;
/// 与离线端到端的模拟标定当量一致（同分辨率现场相机的假设当量），走廊约 121 mm、122 站
const PROBE_MM_PER_PX: f32 = 0.112;
const PROBE_PATH: [[f32; 2]; 2] = [[100.0, 512.0], [1180.0, 512.0]];
const PROBE_MEASURES: usize = 300;
const PROBE_PGMS: usize = 3000;
/// 每张 sync_data 的写盘变体只写这么多张：量不经系统缓存的盘速
const PROBE_SYNCED_PGMS: usize = 300;
/// 三台相机各写自己的目录
const PROBE_WRITERS: usize = 3;
/// 示教图编号：发布清单里的 imageId，探针不用定位模板，只是发布要求有
const TEACH_ID: &str = "capture-engine-0-v1";

const USAGE: &str = "需要环境变量 LYFLOW_CORE_DLL 指向 lyflow_core.dll（已发布的运行库即可，不要求带 lyFlow#32 的构建）。例如（PowerShell，仓库根目录）：
  $env:LYFLOW_CORE_DLL = 'src-tauri\\target\\dependency-cache\\lyflow\\1.1.5\\lyflow_core.dll'
  cargo run --manifest-path src-tauri/Cargo.toml --example throughput_probe
现场机上把它指向安装目录里的 lyflow_core.dll。";

/// 与 vision::eval::synth::Rng 同一个生成器（固定种子，场景可复现）。
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn unit(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }

    fn gauss(&mut self) -> f64 {
        let u = self.unit().max(1e-12);
        let v = self.unit();
        (-2.0 * u.ln()).sqrt() * (2.0 * std::f64::consts::PI * v).cos()
    }
}

fn copy(image: &FrameImage) -> FrameImage {
    FrameImage::new(image.width, image.height, image.pixels.clone())
}

/// 1280×1024：灰 120 ± 3 噪声底，一条 16 px 宽的暗胶条沿 y = 512 横穿示教走廊。
fn probe_scene() -> FrameImage {
    let mut rng = Rng(0x5EED);
    let mut px: Vec<u8> = (0..PROBE_W * PROBE_H).map(|_| (120.0 + 3.0 * rng.gauss()).round() as u8).collect();
    for y in 504..520 {
        for x in 100..=1180 { px[(y * PROBE_W + x) as usize] = 40; }
    }
    FrameImage::new(PROBE_W, PROBE_H, px)
}

/// 单拍照点、无特征定位、走廊横穿整幅图的配方。
fn probe_document() -> RecipeDoc {
    let mut doc = samples().remove(0);
    doc.shots.truncate(1);
    let shot = &mut doc.shots[0];
    shot.path = PROBE_PATH.to_vec();
    shot.mm_per_px = Some(PROBE_MM_PER_PX);
    shot.fixture = None;
    doc.spacing = 1.0;
    doc.detect = DetectParams { search_mm: 3.0, polarity: Polarity::Dark, width_range: [0.8, 2.4] };
    doc.limits = ShotLimits {
        position: Some(JudgeParams { nominal: 0.0, tol_upper: 0.5, tol_lower: 0.5, abs_min: -3.0, abs_max: 3.0, max_excursion_len: 3.0 }),
        width: Some(JudgeParams { nominal: 1.6, tol_upper: 0.5, tol_lower: 0.5, abs_min: 0.5, abs_max: 3.0, max_excursion_len: 3.0 }),
        max_gap_len: 1.5,
        min_present: 0.8,
        gauge_ref: None,
    };
    doc
}

/// 探针写盘的临时目录，随它删除。
struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        Self(std::env::temp_dir().join(format!("gs-throughput-probe-{tag}-{}", std::process::id())))
    }

    /// 同时建好目录（发布会自己建，写盘要先建）。
    fn created(tag: &str) -> Self {
        let dir = Self::new(tag);
        std::fs::create_dir_all(&dir.0).unwrap();
        dir
    }
}

impl Drop for TempDir {
    fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); }
}

/// 与生产同一条加载路径：publish → release::load → Prepared::load → warm。目录随它删除。
fn probe_production(engine: &Arc<Engine>, teach: &FrameImage) -> (Prepared, TempDir) {
    let root = TempDir::new("release");
    let doc = probe_document();
    let view = doc.shots[0].view;
    let recipe = doc.build().unwrap();
    let mut pgm = format!("P5\n{} {}\n255\n", teach.width, teach.height).into_bytes();
    pgm.extend_from_slice(&teach.pixels);
    let graph = serde_json::to_vec(&graphs(&recipe).unwrap()).unwrap();
    let published = publish_release(&root.0, PublishInput {
        recipe: doc, validation_site: ValidationSite::Offline,
        versions: Versions { engine: engine.identity.clone(), graph: GRAPH_VERSION.into() },
        graph: ResourceSource::Bytes(graph),
        shots: vec![ShotInput { k: 0, view, image: Some(ResourceSource::Bytes(pgm)), image_id: Some(TEACH_ID.into()),
            calibration: Some(ResourceSource::Bytes(serde_json::to_vec(&json!({"mmPerPx": PROBE_MM_PER_PX as f64})).unwrap())) }],
    }).unwrap();
    let bundle = load_release(&root.0, &recipe.id, &published.id).unwrap();
    let prepared = Prepared::load(bundle, engine.clone(), &recipe).unwrap();
    warm(&prepared).unwrap();
    (prepared, root)
}

struct Dist { n: usize, mean: f64, p50: f64, p95: f64, max: f64 }

impl Dist {
    /// 最近秩百分位。
    fn of(mut ms: Vec<f64>) -> Self {
        ms.sort_by(f64::total_cmp);
        let n = ms.len();
        let rank = |pct: usize| ms[(n * pct).div_ceil(100) - 1];
        Self { n, mean: ms.iter().sum::<f64>() / n as f64, p50: rank(50), p95: rank(95), max: ms[n - 1] }
    }

    fn line(&self) -> String {
        format!("n={} mean={:.2} p50={:.2} p95={:.2} max={:.2} ms", self.n, self.mean, self.p50, self.p95, self.max)
    }
}

/// workers 个线程共同测 PROBE_MEASURES 帧：返回每帧耗时（ms）与墙钟（s）。
fn probe_measure(prepared: &Prepared, image: &FrameImage, workers: usize) -> (Vec<f64>, f64) {
    let next = AtomicUsize::new(0);
    let wall = Instant::now();
    let per_worker: Vec<Vec<f64>> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..workers).map(|w| {
            let next = &next;
            scope.spawn(move || {
                let mut ms = Vec::new();
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    if i >= PROBE_MEASURES { break ms; }
                    let start = Instant::now();
                    let m = prepared.measure(0, image, &format!("probe-w{workers}-{w}-{i}")).unwrap();
                    ms.push(start.elapsed().as_secs_f64() * 1e3);
                    assert!(m.st.len() > 100 && m.coverage > 0.95, "量到的不是正常路径：站数 {}，有胶比例 {}", m.st.len(), m.coverage);
                }
            })
        }).collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });
    (per_worker.concat(), wall.elapsed().as_secs_f64())
}

/// 生产录制用的 image_file::save_pgm 连续写 count 张（文件名从 first 起），返回每张耗时（ms）。
/// 每张改首行 8 个字节（在胶条之外），免得各张内容完全相同。
fn probe_write(dir: &Path, base: &FrameImage, first: usize, count: usize, synced: bool) -> Vec<f64> {
    use std::io::Write;
    let mut image = copy(base);
    (first..first + count).map(|i| {
        image.pixels[..8].copy_from_slice(&(i as u64).to_le_bytes());
        let path = dir.join(format!("{i:05}.pgm"));
        let start = Instant::now();
        if synced {
            let mut file = std::fs::File::create(&path).unwrap();
            file.write_all(format!("P5\n{} {}\n255\n", image.width, image.height).as_bytes()).unwrap();
            file.write_all(&image.pixels).unwrap();
            file.sync_data().unwrap();
        } else {
            save_pgm(&path, &image).unwrap();
        }
        start.elapsed().as_secs_f64() * 1e3
    }).collect()
}

fn main() {
    let Ok(dll) = std::env::var("LYFLOW_CORE_DLL") else {
        eprintln!("{USAGE}");
        std::process::exit(2);
    };
    let engine = match Engine::load(Path::new(&dll)) {
        Ok(engine) => Arc::new(engine),
        Err(error) => {
            eprintln!("LYFLOW_CORE_DLL = {dll} 加载失败：{error}\n\n{USAGE}");
            std::process::exit(2);
        }
    };

    let teach = probe_scene();
    let cores = std::thread::available_parallelism().map_or(1, |n| n.get());
    println!("[probe] lyFlow {}（{}），逻辑核 {cores}，图像 {PROBE_W}×{PROBE_H}（{} 字节/帧）", engine.version, engine.identity, teach.pixels.len());

    // 一、测量耗时：串行是 p50/p95/max 的依据；并发看 MEASURE_WORKERS 的线性外推是否成立
    let (prepared, _release) = probe_production(&engine, &teach);
    let stations = prepared.measure(0, &teach, "probe-stations").unwrap().st.len();
    println!("[probe] 测点 {stations} 个（间距 1 mm，走廊约 121 mm）");
    let mut levels = vec![1, 2, 3, 4, cores.saturating_sub(2).max(1)];
    levels.sort_unstable();
    levels.dedup();
    let mut serial_p95_ms = 0.0;
    for workers in levels {
        let (ms, wall) = probe_measure(&prepared, &teach, workers);
        let dist = Dist::of(ms);
        if workers == 1 { serial_p95_ms = dist.p95; }
        println!("[probe] 测量 并发 {workers}：{}；墙钟 {wall:.2} s，吞吐 {:.1} fps", dist.line(), PROBE_MEASURES as f64 / wall);
    }
    let needed = (75.0 * serial_p95_ms / 1000.0 / 0.7).ceil();
    println!("[probe] MEASURE_WORKERS = ceil(75 fps × 串行 p95 {serial_p95_ms:.2} ms / 0.7) = {needed}；上限 逻辑核 {cores} − 2 = {}", cores.saturating_sub(2));

    // 二、PGM 写盘：生产录制的 save_pgm
    let bytes_per_file = (teach.pixels.len() + format!("P5\n{PROBE_W} {PROBE_H}\n255\n").len()) as f64;
    let report = |label: &str, ms: Vec<f64>, wall: f64| {
        let files = ms.len();
        println!("[probe] 写盘 {label}：{files} 张 × {:.2} MB，{wall:.2} s，{:.1} MB/s（{:.1} 张/s）；单张 {}",
            bytes_per_file / 1e6, files as f64 * bytes_per_file / 1e6 / wall, files as f64 / wall, Dist::of(ms).line());
    };
    {
        let dir = TempDir::created("pgm-seq");
        let start = Instant::now();
        let ms = probe_write(&dir.0, &teach, 0, PROBE_PGMS, false);
        report("串行（save_pgm，不 sync）", ms, start.elapsed().as_secs_f64());
        let on_disk = std::fs::metadata(dir.0.join("00000.pgm")).unwrap().len() as f64;
        assert_eq!(on_disk, bytes_per_file, "PGM 文件大小不是头 + 全部像素");
    }
    {
        let dir = TempDir::created("pgm-par");
        for cam in 0..PROBE_WRITERS { std::fs::create_dir_all(dir.0.join(format!("cam{cam}"))).unwrap(); }
        let each = PROBE_PGMS / PROBE_WRITERS;
        let start = Instant::now();
        let per_cam: Vec<Vec<f64>> = std::thread::scope(|scope| {
            let handles: Vec<_> = (0..PROBE_WRITERS).map(|cam| {
                let (dir, teach) = (&dir.0, &teach);
                scope.spawn(move || probe_write(&dir.join(format!("cam{cam}")), teach, 0, each, false))
            }).collect();
            handles.into_iter().map(|h| h.join().unwrap()).collect()
        });
        report(&format!("{PROBE_WRITERS} 路并行（每路 {each} 张）"), per_cam.concat(), start.elapsed().as_secs_f64());
    }
    {
        let dir = TempDir::created("pgm-sync");
        let start = Instant::now();
        let ms = probe_write(&dir.0, &teach, 0, PROBE_SYNCED_PGMS, true);
        report("串行（每张 sync_data，不经系统缓存）", ms, start.elapsed().as_secs_f64());
    }
}
