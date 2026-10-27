//! 手动配置数据：按一份段配方候选文档生成若干轮完整的采集轮次，写成候选迁移包（`workspace_transfer_export` 的包格式），
//! 再经 `workspace_transfer_import` 导入成新候选。端到端与长稳测试用它代替示教握手的整圈采集。
//!
//! 轮次记为 `origin: prepared`（不来自现场 PLC，不带 PLC 字段，状态为完成）。每台计划相机的设备配置取自本机
//! `camera_rig_config` 的输出，经 `CameraConfig` 重新序列化，与本机加入标定时的比较口径一致。曝光、增益记默认值。
//!
//! - 合成图（默认）：按配方名义剖面出脉冲（首尾 ±1、弯道 ±2，第 i 轮种子为 seed + i），每个脉冲用
//!   `simimage::render_burst` 为每台计划相机合成一帧；
//! - 样本集（`--sample-set <目录> --part <样本件>`）：第 seq 个帧组取每台相机帧序列的第 seq 帧，帧数少的相机在后面缺帧；
//!   各轮相同。
//!
//! ```powershell
//! cargo run --manifest-path src-tauri/Cargo.toml --offline --locked --example capture_fixture -- `
//!   --doc output/capture-fixture/doc.json --cameras output/capture-fixture/cameras.json `
//!   --out output/capture-fixture/candidate.zip --rounds 14 [--seed 0] [--sample-set <目录> --part p01]
//! ```
//!
//! 完成后在标准输出打印一行 JSON：包路径、配方编号与各轮的编号、创建时间、帧组数和各相机帧数。

use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use gluesight_lib::probe_api::{
    loss_alarms, nominal_frames, render_burst, BurstRender, CameraConfig, FrameImage, PulseProfile,
    Recipe, RecipeDoc, SampleSet, Scenario, SIM_HZ,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

const USAGE: &str = "用法：capture_fixture --doc <候选文档.json> --cameras <camera_rig_config.json> --out <迁移包.zip> --rounds <轮数> [--seed <种子>] [--label <标签>] [--sample-set <样本集目录> --part <样本件>]";
const TRANSFER_FORMAT: u32 = 1;
const MAX_ROUNDS: usize = 200;
const SAMPLE_HZ: f64 = 10.0;

struct Args {
    doc: PathBuf,
    cameras: PathBuf,
    out: PathBuf,
    rounds: usize,
    seed: u64,
    label: String,
    sample: Option<(PathBuf, String)>,
}

fn parse_args() -> Result<Args, String> {
    let rest: Vec<String> = std::env::args().skip(1).collect();
    if rest.len() % 2 != 0 {
        return Err("参数必须成对给出：--名称 值".into());
    }
    let mut named = BTreeMap::new();
    for pair in rest.chunks(2) {
        let name = pair[0]
            .strip_prefix("--")
            .ok_or_else(|| format!("参数 {} 应以 -- 开头", pair[0]))?;
        if named.insert(name.to_string(), pair[1].clone()).is_some() {
            return Err(format!("参数 --{name} 重复"));
        }
    }
    let mut take = |name: &str| named.remove(name);
    let required = |value: Option<String>, name: &str| value.ok_or(format!("缺少 --{name}"));
    let doc = PathBuf::from(required(take("doc"), "doc")?);
    let cameras = PathBuf::from(required(take("cameras"), "cameras")?);
    let out = PathBuf::from(required(take("out"), "out")?);
    let rounds: usize = required(take("rounds"), "rounds")?
        .parse()
        .map_err(|e| format!("--rounds 无效：{e}"))?;
    if !(1..=MAX_ROUNDS).contains(&rounds) {
        return Err(format!("--rounds 需在 1–{MAX_ROUNDS} 之间"));
    }
    let seed = take("seed")
        .map(|s| s.parse().map_err(|e| format!("--seed 无效：{e}")))
        .transpose()?
        .unwrap_or(0);
    let label = take("label").unwrap_or_else(|| "capture_fixture".into());
    if label.trim().is_empty() {
        return Err("--label 不能为空".into());
    }
    let sample = match (take("sample-set"), take("part")) {
        (Some(dir), Some(part)) => Some((PathBuf::from(dir), part)),
        (None, None) => None,
        _ => return Err("--sample-set 与 --part 需同时给出".into()),
    };
    if let Some(name) = named.keys().next() {
        return Err(format!("不认识的参数 --{name}"));
    }
    Ok(Args {
        doc,
        cameras,
        out,
        rounds,
        seed,
        label,
        sample,
    })
}

enum Source {
    Synthetic { recipe: Arc<Recipe>, seed: u64 },
    Samples { set: SampleSet, part: String },
}

/// 一轮的帧：`frames[seq - 1][计划相机下标]`，缺帧为 None；`hz` 为写帧时间戳用的出帧频率。
struct RoundFrames {
    frames: Vec<Vec<Option<FrameImage>>>,
    hz: f64,
}

impl Source {
    fn round(&self, cameras: &[String], index: u64) -> Result<RoundFrames, String> {
        match self {
            Self::Synthetic { recipe, seed } => {
                let profile = PulseProfile {
                    hz: SIM_HZ,
                    segments: nominal_frames(
                        &recipe.path_segments,
                        recipe.expected_total(),
                        SIM_HZ,
                    ),
                    head_tail_jitter: 1,
                    corner_jitter: 2,
                    seed: seed + index,
                };
                let frames = profile
                    .pulses(&[])
                    .iter()
                    .map(|pulse| {
                        std::thread::scope(|scope| {
                            let workers: Vec<_> = (0..cameras.len())
                                .map(|camera_idx| {
                                    let render = BurstRender {
                                        recipe: recipe.clone(),
                                        camera_idx,
                                        seq: pulse.seq,
                                        kind: pulse.kind,
                                        progress: pulse.progress,
                                        scenario: Scenario::Normal,
                                        seed: seed + index,
                                    };
                                    scope.spawn(move || render_burst(&render))
                                })
                                .collect();
                            workers
                                .into_iter()
                                .map(|w| {
                                    w.join().map(Some).map_err(|_| "合成图渲染失败".to_string())
                                })
                                .collect::<Result<Vec<_>, _>>()
                        })
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(RoundFrames { frames, hz: SIM_HZ })
            }
            Self::Samples { set, part } => {
                let shots: Vec<Vec<String>> = cameras
                    .iter()
                    .map(|camera| {
                        set.manifest()
                            .camera_frames(camera)
                            .iter()
                            .map(|shot| shot.shot_id.clone())
                            .collect()
                    })
                    .collect();
                let groups = shots.iter().map(Vec::len).max().unwrap_or(0);
                let frames = (0..groups)
                    .map(|k| {
                        cameras
                            .iter()
                            .zip(&shots)
                            .map(|(camera, shots)| {
                                shots
                                    .get(k)
                                    .map(|shot| set.load_view(part, shot, camera, 1))
                                    .transpose()
                            })
                            .collect::<Result<Vec<_>, _>>()
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(RoundFrames {
                    frames,
                    hz: SAMPLE_HZ,
                })
            }
        }
    }
}

struct Package {
    zip: ZipWriter<BufWriter<File>>,
    files: Vec<Value>,
}

impl Package {
    fn add(&mut self, path: &str, bytes: &[u8]) -> Result<(), String> {
        let level = if path.ends_with(".pgm") { 1 } else { 6 };
        let options = SimpleFileOptions::default()
            .compression_method(CompressionMethod::Deflated)
            .compression_level(Some(level));
        self.zip
            .start_file(path, options)
            .map_err(|e| e.to_string())?;
        self.zip.write_all(bytes).map_err(|e| e.to_string())?;
        self.files.push(json!({
            "path": path,
            "bytes": bytes.len(),
            "sha256": format!("{:x}", Sha256::digest(bytes)),
        }));
        Ok(())
    }
}

fn pgm(image: &FrameImage) -> Vec<u8> {
    let mut bytes = format!("P5\n{} {}\n255\n", image.width, image.height).into_bytes();
    bytes.extend_from_slice(&image.pixels);
    bytes
}

fn read_json(path: &Path) -> Result<Value, String> {
    let text =
        std::fs::read_to_string(path).map_err(|e| format!("读取 {} 失败：{e}", path.display()))?;
    serde_json::from_str(text.trim_start_matches('\u{feff}'))
        .map_err(|e| format!("{} 不是有效 JSON：{e}", path.display()))
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

/// 写一轮：原图、frames.jsonl 与 round.json（统计按已写的帧算，满足轮次记录的自洽核对）。
#[allow(clippy::too_many_arguments)]
fn write_round(
    package: &mut Package,
    doc: &RecipeDoc,
    cameras: &[String],
    configs: &[Value],
    label: &str,
    round_id: &str,
    created_at: i64,
    round: RoundFrames,
) -> Result<Value, String> {
    let group_count = round.frames.len() as u64;
    if group_count == 0 {
        return Err("一轮没有帧组：配方剖面或样本集为空".into());
    }
    let period_ms = 1000.0 / round.hz;
    let mut saved = vec![BTreeSet::new(); cameras.len()];
    let mut lines = String::new();
    for (k, images) in round.frames.into_iter().enumerate() {
        let seq = k as u64 + 1;
        for (camera, image) in images.into_iter().enumerate() {
            let Some(image) = image else { continue };
            let path = format!("{}/{seq:06}.pgm", cameras[camera]);
            package.add(&format!("rounds/{round_id}/{path}"), &pgm(&image))?;
            saved[camera].insert(seq);
            lines.push_str(
                &json!({
                    "camera": camera,
                    "seq": seq,
                    "frameCounter": seq,
                    "triggerCounter": null,
                    "path": path,
                    "width": image.width,
                    "height": image.height,
                    "ts": created_at + (k as f64 * period_ms).round() as i64,
                })
                .to_string(),
            );
            lines.push('\n');
        }
    }
    let camera_frames: Vec<u32> = saved.iter().map(|s| s.len() as u32).collect();
    let mut missing: Vec<(u64, usize)> = saved
        .iter()
        .enumerate()
        .flat_map(|(camera, seqs)| {
            (1..=group_count)
                .filter(|seq| !seqs.contains(seq))
                .map(move |seq| (seq, camera))
        })
        .collect();
    missing.sort_unstable();
    let mut incomplete: Vec<u64> = missing.iter().map(|&(seq, _)| seq).collect();
    incomplete.dedup();
    let record = json!({
        "format": 3,
        "roundId": round_id,
        "recipeId": doc.id,
        "cameras": cameras.iter().zip(configs).zip(&camera_frames).map(|((camera, config), frames)| json!({
            "cameraId": camera,
            "cameraConfig": config,
            "imaging": {"exposureUs": 60.0, "gainDb": 6.0},
            "deviceSession": 1,
            "receivedCount": frames,
        })).collect::<Vec<_>>(),
        "state": "complete",
        "plcShotCount": null,
        "plcPlanVersion": null,
        "teachingRoundId": null,
        "deviceTriggers": null,
        "groupCount": group_count,
        "cameraFrames": camera_frames,
        "incompleteGroups": incomplete,
        "missingFrames": missing.iter().map(|&(seq, camera)| json!({"camera": camera, "seq": seq})).collect::<Vec<_>>(),
        "lossAlarms": loss_alarms(&camera_frames),
        "alarms": [],
        "createdAt": created_at,
        "endedAt": created_at + (group_count as f64 * period_ms).round() as i64,
        "error": null,
        "failure": null,
        "simulated": true,
        "origin": {"kind": "prepared", "label": label},
    });
    package.add(&format!("rounds/{round_id}/frames.jsonl"), lines.as_bytes())?;
    package.add(
        &format!("rounds/{round_id}/round.json"),
        &serde_json::to_vec_pretty(&record).map_err(|e| e.to_string())?,
    )?;
    Ok(json!({
        "roundId": round_id,
        "createdAt": created_at,
        "groupCount": group_count,
        "cameraFrames": camera_frames,
    }))
}

fn run(args: Args) -> Result<Value, String> {
    let doc: RecipeDoc = serde_json::from_value(read_json(&args.doc)?)
        .map_err(|e| format!("候选文档格式无效：{e}"))?;
    if !doc.is_burst() {
        return Err("候选文档不是段配方".into());
    }
    let recipe = Arc::new(doc.build().map_err(|e| format!("候选文档无法编译：{e}"))?);
    let cameras = doc.plan_cameras();
    let rig: Vec<CameraConfig> = serde_json::from_value(read_json(&args.cameras)?)
        .map_err(|e| format!("相机配置格式无效：{e}"))?;
    let configs = cameras
        .iter()
        .map(|camera| {
            let config = rig
                .iter()
                .find(|c| &c.id == camera)
                .ok_or_else(|| format!("相机配置里没有计划相机 {camera}"))?;
            serde_json::to_value(config).map_err(|e| e.to_string())
        })
        .collect::<Result<Vec<_>, _>>()?;
    let source = match args.sample {
        Some((dir, part)) => {
            let set = SampleSet::load(&dir)?;
            if set.part(&part).is_none() {
                return Err(format!("样本集没有样本件 {part}"));
            }
            if let Some(camera) = cameras.iter().find(|c| !set.has_camera(c)) {
                return Err(format!("样本集没有计划相机 {camera}"));
            }
            Source::Samples { set, part }
        }
        None => Source::Synthetic {
            recipe,
            seed: args.seed,
        },
    };
    if let Some(parent) = args.out.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let mut temporary = args.out.clone().into_os_string();
    temporary.push(".tmp");
    let temporary = PathBuf::from(temporary);
    let file =
        File::create(&temporary).map_err(|e| format!("创建 {} 失败：{e}", temporary.display()))?;
    let mut package = Package {
        zip: ZipWriter::new(BufWriter::new(file)),
        files: Vec::new(),
    };
    let base = now_ms();
    let result = (|| {
        let workspace = json!({
            "doc": doc,
            "baseRevision": null,
            "revision": 1,
            "samples": [],
            "sampleBank": [],
            "validation": null,
            "pending": null,
            "publish": {"state": "draft"},
            "updatedAt": base,
        });
        package.add(
            "workspace/workspace.json",
            &serde_json::to_vec_pretty(&workspace).map_err(|e| e.to_string())?,
        )?;
        let mut rounds = Vec::with_capacity(args.rounds);
        for index in 0..args.rounds {
            let round_id = format!("capture-{base}-{index}");
            let frames = source.round(&cameras, index as u64)?;
            rounds.push(write_round(
                &mut package,
                &doc,
                &cameras,
                &configs,
                &args.label,
                &round_id,
                base + index as i64,
                frames,
            )?);
        }
        let manifest = json!({
            "format": TRANSFER_FORMAT,
            "kind": "candidate",
            "recipeId": doc.id,
            "name": doc.name,
            "productCode": doc.product_code,
            "version": doc.version,
            "exportedAt": base,
            "appVersion": env!("CARGO_PKG_VERSION"),
            "rounds": rounds.iter().map(|r| r["roundId"].clone()).collect::<Vec<_>>(),
            "notes": [format!("手动配置数据（{}）：capture_fixture 生成 {} 轮", args.label, rounds.len())],
            "files": std::mem::take(&mut package.files),
        });
        package
            .zip
            .start_file(
                "manifest.json",
                SimpleFileOptions::default().compression_method(CompressionMethod::Deflated),
            )
            .map_err(|e| e.to_string())?;
        package
            .zip
            .write_all(&serde_json::to_vec_pretty(&manifest).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
        package
            .zip
            .finish()
            .map_err(|e| e.to_string())?
            .into_inner()
            .map_err(|e| e.to_string())?
            .sync_all()
            .map_err(|e| e.to_string())?;
        Ok(rounds)
    })();
    let rounds = match result {
        Ok(rounds) => rounds,
        Err(error) => {
            let _ = std::fs::remove_file(&temporary);
            return Err(error);
        }
    };
    if args.out.exists() {
        std::fs::remove_file(&args.out)
            .map_err(|e| format!("覆盖 {} 失败：{e}", args.out.display()))?;
    }
    std::fs::rename(&temporary, &args.out).map_err(|e| e.to_string())?;
    Ok(json!({
        "path": args.out.display().to_string(),
        "recipeId": doc.id,
        "rounds": rounds,
    }))
}

fn main() {
    let outcome = parse_args().and_then(run);
    match outcome {
        Ok(summary) => println!("{summary}"),
        Err(error) => {
            eprintln!("{error}\n{USAGE}");
            std::process::exit(1);
        }
    }
}
