//! 手动配置数据（docs/decisions/recipe-build-flow.md「手动配置数据」）：按配方生成若干件仿历史录制件，与真机维护采集件同格式——
//! 在真机录制根下写录制目录（`part.json` 带 `partMode`、`devices`、`plc`，原图 `{相机}_s{seq:06}.pgm`），在项目记录库写
//! `parts` 行、逐帧记录与录制证据，模式为维护采集（不进当日统计）。端到端与长稳测试用它代替现场采集，再经历史件选择器
//! （`workspace_history_candidates`）选示教件与标定件，走与现场相同的导入路径。软件运行中写入也可以，选择器每次重新查库。
//!
//! `--doc` 是录制这些件时生效的生产配方（通常是先发布的采集版，`recipe_doc`）：记录引用它的修订号 `{编号}-v{版本}`，
//! 必须是软件已分配过的版本，否则以后同修订号的真实记录会因快照不同被拒；记录库已有该修订号的另一份快照时拒绝写入。
//! `--db` 须是软件已建好的记录库（`<资料根目录>/projects/<项目编号>/inspection.db`），`--records` 是真机录制根（`records_list`
//! 的 root）。每台计划相机的设备配置取自本机 `camera_rig_config` 的输出，经 `CameraConfig` 重新序列化，与软件核对历史件时
//! 的口径一致；曝光、增益记默认值。来源标记：`part.json` 带 `fixture: {label}`，SN 以 `FIXTURE-` 开头，记录原因注明手动配置数据。
//!
//! - 合成图（默认）：按配方名义剖面出脉冲（首尾 ±1、弯道 ±2，第 i 件种子为 seed + i），每个脉冲用
//!   `simimage::render_burst_span` 为每台计划相机合成一帧；
//!   - 起涂、收尾（D-SB-50）：`--onset` / `--tail` 给基准帧组数，`--onset-jitter` / `--tail-jitter` 给抖动，默认都为 0（不模拟）。
//!     第 i 件按种子 seed + i 在 [基准, 基准 + 抖动] 内取起涂帧组数 h、收尾帧组数 t：前 h 个帧组的前一半（向上取整）无胶，
//!     之后胶条从胶嘴沿走廊往外逐帧变长，只铺到走廊的一部分；末 t 个帧组对称，胶尾逐帧离开胶嘴，最后一半无胶。
//!     h + t 须小于本件帧组数。h、t 是胶条没铺满走廊的帧组数，标定按检测项判成形，建议值可以小于它们。
//! - 样本集（`--sample-set <目录> --part <样本件>`）：第 seq 个帧组取每台相机帧序列的第 seq 帧，帧数少的相机在后面缺帧，
//!   清单里有这一帧但没有图的就地缺帧、不前移后面的帧（有缺帧的件不能选作示教件或标定件）；各件相同，不能与起涂、收尾参数同用。
//!
//! 中途失败时已写完的件保留。
//!
//! ```powershell
//! cargo run --manifest-path src-tauri/Cargo.toml --offline --locked --example capture_fixture -- `
//!   --doc output/capture-fixture/doc.json --cameras output/capture-fixture/cameras.json `
//!   --db <资料根目录>/projects/<项目编号>/inspection.db --records <真机录制根> --parts 13 [--seed 0] [--label <标签>]
//!   [--sample-set <目录> --part p01] [--onset 4 --onset-jitter 2 --tail 2 --tail-jitter 0]
//! ```
//!
//! 完成后在标准输出打印一行 JSON：配方编号、修订号、标签与各件的记录 ID（recordId）、cycleId、SN、时间、录制目录、帧组数和
//! 各相机帧数；合成图另带各件的起涂帧组数 `onset` 与收尾帧组数 `tail`。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use gluesight_lib::probe_api::{
    nominal_frames, render_burst_span, BurstRender, CameraConfig, FixtureFrames, FixtureSite,
    PulseProfile, Recipe, RecipeDoc, SampleSet, Scenario, SIM_HZ, SN_PREFIX,
};
use serde_json::{json, Value};

const USAGE: &str = "用法：capture_fixture --doc <录制时生效的生产配方.json> --cameras <camera_rig_config.json> --db <项目目录>/inspection.db --records <真机录制根> --parts <件数> [--seed <种子>] [--label <标签>] [--sample-set <样本集目录> --part <样本件>] [--onset <帧组数> --onset-jitter <帧组数> --tail <帧组数> --tail-jitter <帧组数>]";
const MAX_PARTS: usize = 200;
const SAMPLE_HZ: f64 = 10.0;

struct Args {
    doc: PathBuf,
    cameras: PathBuf,
    db: PathBuf,
    records: PathBuf,
    parts: usize,
    seed: u64,
    label: String,
    sample: Option<(PathBuf, String)>,
    onset: Ramp,
    tail: Ramp,
}

/// 起涂或收尾的帧组数：每件在 [base, base + jitter] 内取。
#[derive(Clone, Copy, Default)]
struct Ramp {
    base: u32,
    jitter: u32,
}

impl Ramp {
    /// 第 `stream` 路的取值（起涂、收尾各一路），`seed` 即该件种子 seed + i。
    fn draw(self, seed: u64, stream: u64) -> u32 {
        let mut z = seed.wrapping_add(stream.wrapping_add(1).wrapping_mul(0x9E37_79B9_7F4A_7C15));
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^= z >> 31;
        self.base + (z % (u64::from(self.jitter) + 1)) as u32
    }
}

/// 第 k 个帧组（0 起，共 n 个）胶条沿走廊的覆盖区间（从出胶点量起的弧长占比）：起涂区前一半（向上取整）无胶，
/// 之后从胶嘴往外逐帧变长；收尾区对称，胶尾逐帧离开胶嘴，最后一半无胶；其余帧组整条胶。
fn bead_span(k: u32, n: u32, onset: u32, tail: u32) -> [f32; 2] {
    let from_end = n - 1 - k;
    if k < onset {
        let dry = onset.div_ceil(2);
        if k < dry {
            return [0.0, 0.0];
        }
        [0.0, (k - dry + 1) as f32 / (onset - dry + 1) as f32]
    } else if from_end < tail {
        let dry = tail.div_ceil(2);
        if from_end < dry {
            return [0.0, 0.0];
        }
        [(tail - from_end) as f32 / (tail - dry + 1) as f32, 1.0]
    } else {
        [0.0, 1.0]
    }
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
    let db = PathBuf::from(required(take("db"), "db")?);
    let records = PathBuf::from(required(take("records"), "records")?);
    let parts: usize = required(take("parts"), "parts")?
        .parse()
        .map_err(|e| format!("--parts 无效：{e}"))?;
    if !(1..=MAX_PARTS).contains(&parts) {
        return Err(format!("--parts 需在 1–{MAX_PARTS} 之间"));
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
    let [onset, onset_jitter, tail, tail_jitter] = ["onset", "onset-jitter", "tail", "tail-jitter"]
        .map(|name| {
            take(name).map(|v| v.parse::<u32>().map_err(|e| format!("--{name} 无效：{e}")))
        });
    if sample.is_some()
        && [&onset, &onset_jitter, &tail, &tail_jitter]
            .iter()
            .any(|v| v.is_some())
    {
        return Err("起涂、收尾参数只用于合成图，不能与 --sample-set 同用".into());
    }
    let value = |v: Option<Result<u32, String>>| v.transpose().map(Option::unwrap_or_default);
    let onset = Ramp {
        base: value(onset)?,
        jitter: value(onset_jitter)?,
    };
    let tail = Ramp {
        base: value(tail)?,
        jitter: value(tail_jitter)?,
    };
    if let Some(name) = named.keys().next() {
        return Err(format!("不认识的参数 --{name}"));
    }
    Ok(Args {
        doc,
        cameras,
        db,
        records,
        parts,
        seed,
        label,
        sample,
        onset,
        tail,
    })
}

enum Source {
    Synthetic {
        recipe: Arc<Recipe>,
        seed: u64,
        onset: Ramp,
        tail: Ramp,
    },
    Samples {
        set: SampleSet,
        part: String,
    },
}

impl Source {
    /// 第 index 件的帧与合成图的起涂、收尾帧组数。
    fn part(
        &self,
        cameras: &[String],
        index: u64,
    ) -> Result<(FixtureFrames, Option<[u32; 2]>), String> {
        match self {
            Self::Synthetic {
                recipe,
                seed,
                onset,
                tail,
            } => {
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
                let pulses = profile.pulses(&[]);
                let n = pulses.len() as u32;
                let ends = [onset.draw(seed + index, 0), tail.draw(seed + index, 1)];
                if ends[0] + ends[1] >= n {
                    return Err(format!(
                        "第 {} 件起涂 {} + 收尾 {} 帧组不少于本件帧组数 {n}",
                        index + 1,
                        ends[0],
                        ends[1]
                    ));
                }
                let frames = pulses
                    .iter()
                    .map(|pulse| {
                        let span = bead_span(pulse.seq as u32 - 1, n, ends[0], ends[1]);
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
                                    scope.spawn(move || render_burst_span(&render, span))
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
                Ok((FixtureFrames { frames, hz: SIM_HZ }, Some(ends)))
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
                            .map(|(camera, shots)| match shots.get(k) {
                                Some(shot) if set.image_path(part, shot, camera, 1).is_ok() => {
                                    set.load_view(part, shot, camera, 1).map(Some)
                                }
                                _ => Ok(None),
                            })
                            .collect::<Result<Vec<_>, _>>()
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                Ok((
                    FixtureFrames {
                        frames,
                        hz: SAMPLE_HZ,
                    },
                    None,
                ))
            }
        }
    }
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

fn run(args: Args) -> Result<Value, String> {
    let doc: RecipeDoc = serde_json::from_value(read_json(&args.doc)?)
        .map_err(|e| format!("配方文档格式无效：{e}"))?;
    if !doc.is_burst() {
        return Err("配方文档不是段配方".into());
    }
    let recipe = Arc::new(doc.build().map_err(|e| format!("配方文档无法编译：{e}"))?);
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
    let site = FixtureSite::open(
        &args.db,
        &args.records,
        (*recipe).clone(),
        configs,
        &args.label,
    )?;
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
            recipe: recipe.clone(),
            seed: args.seed,
            onset: args.onset,
            tail: args.tail,
        },
    };
    let base = now_ms();
    let mut parts: Vec<Value> = Vec::with_capacity(args.parts);
    for index in 0..args.parts {
        let sn = format!("{SN_PREFIX}{base}-{:03}", index + 1);
        let written = source
            .part(&cameras, index as u64)
            .and_then(|(frames, ends)| {
                let mut part =
                    serde_json::to_value(site.write(&sn, frames)?).map_err(|e| e.to_string())?;
                if let Some([onset, tail]) = ends {
                    part["onset"] = json!(onset);
                    part["tail"] = json!(tail);
                }
                Ok(part)
            });
        match written {
            Ok(part) => parts.push(part),
            Err(error) if parts.is_empty() => return Err(error),
            Err(error) => {
                let kept: Vec<&str> = parts.iter().filter_map(|p| p["sn"].as_str()).collect();
                return Err(format!(
                    "{error}\n已写完的 {} 件保留：{}",
                    kept.len(),
                    kept.join("、")
                ));
            }
        }
    }
    Ok(json!({
        "recipeId": doc.id,
        "revision": recipe.revision_id,
        "label": args.label,
        "parts": parts,
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
