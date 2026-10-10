use super::*;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::judge::{self, Judgement, PointState, Verdict};
use crate::measure::{ST_GAP, ST_OK};
use crate::recipe::{DetectParams, Polarity, RecipeDoc};
use crate::release::{self, PublishInput, ResourceSource, ShotInput, Versions};
use crate::sim::Scenario;
use crate::simimage::{self, PoseError, SIM_SIZE};

const SEED: u64 = 20261010;

struct Fixtures {
    normal: Vec<Vec<Arc<FrameImage>>>,
    gap: Vec<Vec<Arc<FrameImage>>>,
    empty: Vec<Vec<Arc<FrameImage>>>,
    manifest: Value,
}

fn pgm(image: &FrameImage) -> Vec<u8> {
    let mut bytes = format!("P5\n{} {}\n255\n", image.width, image.height).into_bytes();
    bytes.extend_from_slice(&image.pixels);
    bytes
}

fn artifact(path: &Path) -> Value {
    let path = path.canonicalize().unwrap();
    let bytes = std::fs::metadata(&path).unwrap().len();
    json!({"path": path, "bytes": bytes})
}

fn software_snapshot(source: &Path, destination: &Path) -> Value {
    std::fs::create_dir_all(destination.parent().unwrap()).unwrap();
    assert!(!destination.exists(), "Software evidence must not overwrite an existing snapshot");
    std::fs::copy(source, destination).unwrap();
    artifact(destination)
}

fn write_json(path: &Path, value: &Value) {
    std::fs::write(path, serde_json::to_vec_pretty(value).unwrap()).unwrap();
}

fn recipe(view_count: u8, clean: bool) -> RecipeDoc {
    let mut doc = crate::recipe::samples().remove(1);
    doc.id = format!("P0-REGRESSION-{view_count}V{}", if clean { "-CLEAN" } else { "" });
    doc.name = format!("P0 1280x1024 {view_count} 视角回归");
    doc.spacing = 1.0;
    doc.detect = DetectParams {
        search_mm: 8.0,
        polarity: Polarity::Dark,
        width_range: [1.5, 6.5],
    };
    for (k, shot) in doc.shots.iter_mut().enumerate() {
        let y = 432.0 + 32.0 * k as f32;
        shot.camera = "cam1".into();
        shot.view = if view_count == 3 { [1, 2, 3, 1][k] } else { 1 };
        shot.path = vec![[1040.0, y], [240.0, y]];
        shot.mm_per_px = Some(0.125);
        shot.detect = None;
        shot.limits = None;
        shot.skip = false;
    }
    doc
}

fn clean_views(recipe: &Recipe, k: usize, scenario: Option<Scenario>, view_count: u8) -> Vec<Arc<FrameImage>> {
    let shot = &recipe.shots[k];
    let gap = scenario.map(|s| s.gap_points(recipe)).unwrap_or_default();
    let segment = recipe.shot_segment(k).unwrap();
    let step = recipe.spacing / shot.mm_per_px.unwrap();
    let gap_range = gap.first().zip(gap.last()).filter(|(first, _)| **first >= segment.first && **first < segment.first + segment.count)
        .map(|(first, last)| (((first - segment.first) as f32 * step), ((last - segment.first + 1) as f32 * step)));
    (1..=view_count).map(|view| {
        let mut pixels = vec![210; 1280 * 1024];
        if view == shot.view && scenario.is_some() {
            let center_y = shot.path[0][1] as usize;
            for y in center_y - 16..center_y + 16 {
                for x in 220..1061 {
                    let distance = shot.path[0][0] - x as f32;
                    if !gap_range.is_some_and(|(a, b)| distance >= a && distance < b) {
                        pixels[y * 1280 + x] = 38;
                    }
                }
            }
        }
        Arc::new(FrameImage::new(1280, 1024, pixels))
    }).collect()
}

fn fixtures(root: &Path, recipe: &Recipe, view_count: u8, clean: bool) -> Fixtures {
    std::fs::create_dir_all(root).unwrap();
    let mut scenarios = Vec::new();
    let mut sets = Vec::new();
    for (name, scenario) in [
        ("normal", Some(Scenario::Normal)),
        ("partial_gap", Some(Scenario::Gap)),
        ("whole_empty", None),
    ] {
        let mut shots = Vec::new();
        let mut images = Vec::new();
        for (k, shot) in recipe.shots.iter().enumerate() {
            let seed = SEED + k as u64;
            let views = if clean { clean_views(recipe, k, scenario, view_count) } else { match scenario {
                Some(scenario) => simimage::render_views(
                    recipe,
                    k,
                    scenario,
                    PoseError::default(),
                    seed,
                    view_count,
                )
                .unwrap(),
                None => simimage::background_views(view_count, seed).unwrap(),
            }};
            assert_eq!(views.len(), view_count as usize);
            let mut records = Vec::new();
            for (v, image) in views.iter().enumerate() {
                assert_eq!([image.width, image.height], [1280, 1024]);
                assert_eq!(image.pixels.len(), 1280 * 1024);
                let selected = v + 1 == shot.view as usize;
                let mid = shot.path_at(shot.path_len_px() * 0.25);
                let pixel = image.pixels[mid[1] as usize * image.width as usize + mid[0] as usize];
                if selected && scenario.is_some() {
                    assert!(pixel < 70);
                } else {
                    assert!(image.pixels.iter().all(|&p| p > 120));
                }
                let path = root.join(format!("{name}-p{}-v{}.pgm", k + 1, v + 1));
                std::fs::write(&path, pgm(image)).unwrap();
                records.push(
                    json!({"view": v + 1, "selected": selected, "image": artifact(&path),
                    "probePx": [mid[0] as u32, mid[1] as u32], "probePixel": pixel}),
                );
            }
            shots.push(json!({"k": k, "shotId": shot.id, "poseId": shot.pose_id,
                "camera": shot.camera, "selectedView": shot.view, "seed": seed, "images": records}));
            images.push(views);
        }
        scenarios.push(json!({"name": name, "shots": shots}));
        sets.push(images);
    }
    let manifest = json!({"schemaVersion": 1, "source": if clean { "production::regression::clean_views: independent uniform 210 Gray8 background, 38 Gray8 straight bead, exact 32-pixel width; no texture, noise, nozzle or pose shift" } else { "crate::simimage::render_views/background_views" },
        "fixtureStyle": if clean { "clean_step_edge" } else { "simulated_metal" },
        "accuracyBoundary": "A clean synthetic fixture is a performance/control case only. It does not repair or supersede P0-09 failures on textured metal backgrounds.",
        "imageSize": SIM_SIZE, "format": "8-bit row-major grayscale, P5 PGM, no resizing",
        "nominalWidthMm": 4.0, "mmPerPx": 0.125, "pathLengthPx": 800, "stationsPerShot": 101,
        "partialGap": {"shot": 1, "removedStations": Scenario::Gap.gap_points(recipe), "maxGapLenMm": 6.0},
        "wholeEmpty": "background only in every view; no dark bead in the detection region",
        "fixtureReuse": "The same deterministic full-resolution pixels are reused across parts; rendering, camera transport and acquisition are outside the timed loop.",
        "scenarios": scenarios});
    let mut sets = sets.into_iter();
    Fixtures {
        normal: sets.next().unwrap(),
        gap: sets.next().unwrap(),
        empty: sets.next().unwrap(),
        manifest,
    }
}

fn distribution(values: &[f64]) -> Value {
    assert!(!values.is_empty());
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    let percentile = |p: f64| sorted[((sorted.len() as f64 * p).ceil() as usize).saturating_sub(1)];
    json!({"count": values.len(), "p50": percentile(0.50), "p95": percentile(0.95),
        "max": sorted[sorted.len() - 1], "min": sorted[0],
        "mean": values.iter().sum::<f64>() / values.len() as f64, "unit": "ms", "percentile": "nearest rank"})
}

fn milliseconds(start: Instant) -> f64 {
    start.elapsed().as_secs_f64() * 1000.0
}

#[cfg(windows)]
fn memory() -> Value {
    #[repr(C)]
    #[derive(Default)]
    struct Counters {
        cb: u32,
        page_fault_count: u32,
        peak_working_set_size: usize,
        working_set_size: usize,
        quota_peak_paged_pool_usage: usize,
        quota_paged_pool_usage: usize,
        quota_peak_non_paged_pool_usage: usize,
        quota_non_paged_pool_usage: usize,
        pagefile_usage: usize,
        peak_pagefile_usage: usize,
        private_usage: usize,
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn GetCurrentProcess() -> *mut std::ffi::c_void;
    }
    #[link(name = "psapi")]
    extern "system" {
        fn GetProcessMemoryInfo(
            process: *mut std::ffi::c_void,
            counters: *mut Counters,
            size: u32,
        ) -> i32;
    }
    let mut counters = Counters::default();
    counters.cb = std::mem::size_of::<Counters>() as u32;
    let ok = unsafe {
        GetProcessMemoryInfo(
            GetCurrentProcess(),
            &mut counters,
            std::mem::size_of::<Counters>() as u32,
        )
    };
    assert_ne!(
        ok,
        0,
        "GetProcessMemoryInfo failed: {}",
        std::io::Error::last_os_error()
    );
    json!({"source": "GetProcessMemoryInfo/PROCESS_MEMORY_COUNTERS_EX", "workingSetBytes": counters.working_set_size,
        "privateBytes": counters.private_usage, "peakWorkingSetBytes": counters.peak_working_set_size})
}

#[cfg(not(windows))]
fn memory() -> Value {
    json!({"source": "unavailable", "workingSetBytes": null, "privateBytes": null})
}

fn memory_sample(part: usize, since: Instant) -> Value {
    let mut sample = memory();
    sample["part"] = json!(part);
    sample["elapsedMs"] = json!(milliseconds(since));
    sample
}

fn memory_trend(samples: &[Value]) -> Value {
    let trend = |key: &str| -> Value {
        let values: Vec<_> = samples.iter().filter_map(|s| s[key].as_u64()).collect();
        if values.is_empty() {
            return Value::Null;
        }
        let first = values[0];
        let last = values[values.len() - 1];
        let x: Vec<_> = samples
            .iter()
            .filter(|s| s[key].is_u64())
            .map(|s| s["part"].as_u64().unwrap() as f64)
            .collect();
        let mean_x = x.iter().sum::<f64>() / x.len() as f64;
        let mean_y = values.iter().map(|&n| n as f64).sum::<f64>() / values.len() as f64;
        let denominator = x.iter().map(|n| (n - mean_x).powi(2)).sum::<f64>();
        let slope = if denominator > 0.0 {
            x.iter()
                .zip(&values)
                .map(|(&x, &y)| (x - mean_x) * (y as f64 - mean_y))
                .sum::<f64>()
                / denominator
        } else {
            0.0
        };
        json!({"startBytes": first, "endBytes": last, "deltaBytes": last as i128 - first as i128,
            "minBytes": values.iter().min(), "maxBytes": values.iter().max(), "linearSlopeBytesPerPart": slope})
    };
    json!({"workingSet": trend("workingSetBytes"), "private": trend("privateBytes"), "samples": samples,
        "interpretation": "In-process samples every 10 parts after fixture allocation and warmup. Per-part JSON evidence is streamed to disk and dropped before sampling. Timing vectors are preallocated; the bounded memory-sample log, writer buffer, allocator caches and DLL state are included. This finite run alone does not prove absence of a memory leak."})
}

fn measured_part(prepared: &Prepared, images: &[Vec<Arc<FrameImage>>], run_prefix: &str) -> Value {
    let recipe = &prepared.recipe;
    let mut table = vec![PointState::Pending; recipe.point_count()];
    let mut calls = Vec::new();
    let mut errors = Vec::new();
    let start = Instant::now();
    let mut last_frame_at = start;
    for (k, shot) in recipe.shots.iter().enumerate() {
        let image = &images[k][shot.view as usize - 1];
        let submitted = Instant::now();
        if k + 1 == recipe.shot_count() {
            last_frame_at = submitted;
        }
        let result = prepared.measure(k, image, &format!("{run_prefix}-p{k}"));
        let wall_ms = milliseconds(submitted);
        match result {
            Ok(m) => {
                let owned: Vec<_> = recipe.owned_points(k).collect();
                assert_eq!(m.idx.len(), 101);
                assert_eq!(m.idx.iter().map(|&i| i as usize).collect::<Vec<_>>(), owned);
                assert_eq!([m.d.len(), m.w.len(), m.st.len()], [owned.len(); 3]);
                for (i, &j) in owned.iter().enumerate() {
                    table[j] = match m.st[i] {
                        ST_OK => {
                            assert!(m.d[i].is_finite() && m.w[i].is_finite());
                            PointState::Measured {
                                d: m.d[i],
                                w: m.w[i],
                            }
                        }
                        ST_GAP => PointState::Gap,
                        other => panic!("Unexpected native status {other}"),
                    };
                }
                let widths: Vec<_> = m.w.iter().copied().filter(|w| w.is_finite()).collect();
                calls.push(json!({"k": k, "shotId": shot.id, "camera": shot.camera, "view": shot.view,
                    "size": [image.width, image.height], "preparedMeasureMs": wall_ms,
                    "engineRunIoParseMs": m.ms, "coverage": m.coverage, "stations": m.idx.len(),
                    "gapStations": m.st.iter().filter(|&&s| s == ST_GAP).count(),
                    "widthMinMm": widths.iter().copied().reduce(f32::min), "widthMaxMm": widths.iter().copied().reduce(f32::max),
                    "error": null}));
            }
            Err(error) => {
                for j in recipe.owned_points(k) {
                    table[j] = PointState::Invalid;
                }
                calls.push(
                    json!({"k": k, "shotId": shot.id, "camera": shot.camera, "view": shot.view,
                    "size": [image.width, image.height], "preparedMeasureMs": wall_ms,
                    "engineRunIoParseMs": null, "error": error}),
                );
                errors.push(error);
            }
        }
    }
    let result = if errors.is_empty() {
        judge::judge(recipe, &table)
    } else {
        Judgement::error(judge::fault::PROCESS_TIMEOUT, errors.join("; "))
    };
    let judged_at = Instant::now();
    json!({"calls": calls, "judgement": result, "serialPartMeasureAndJudgeMs": judged_at.duration_since(start).as_secs_f64() * 1000.0,
        "lastSelectedFrameToJudgeMs": judged_at.duration_since(last_frame_at).as_secs_f64() * 1000.0, "measurementErrors": errors.len(),
        "decisionSource": if errors.is_empty() { "crate::judge::judge" } else { "Judgement::error(99), matching cycle::Part::apply_result native-error precedence" }})
}

fn assert_judgement(part: &Value, verdict: Verdict, fault: u16) {
    assert_eq!(
        part["judgement"]["verdict"],
        serde_json::to_value(verdict).unwrap(),
        "{part}"
    );
    assert_eq!(part["judgement"]["plcCode"], verdict.plc_code());
    assert_eq!(part["judgement"]["faultCode"], fault);
}

fn run_case(
    root: &Path,
    engine: Arc<Engine>,
    view_count: u8,
    parts: usize,
    engine_already_used: bool,
    clean: bool,
) -> Value {
    let root = root.join(format!("{view_count}-view"));
    std::fs::create_dir_all(&root).unwrap();
    let doc = recipe(view_count, clean);
    let recipe = doc.build().unwrap();
    assert_eq!(recipe.shot_count(), 4);
    assert_eq!(recipe.cameras(), ["cam1".to_string()]);
    let rendering = Instant::now();
    let fixtures = fixtures(&root.join("fixtures"), &recipe, view_count, clean);
    let fixture_render_ms = milliseconds(rendering);
    let fixture_manifest = root.join("fixtures.json");
    write_json(&fixture_manifest, &fixtures.manifest);
    let published_at = Instant::now();
    let bundle = release::publish(&root.join("releases"), PublishInput {
        recipe: doc,
        versions: Versions { engine: engine.version.clone(), graph: GRAPH_VERSION.into() },
        graph: ResourceSource::Bytes(serde_json::to_vec(&graphs(&recipe).unwrap()).unwrap()),
        shots: recipe.shots.iter().enumerate().map(|(k, shot)| ShotInput { k,
            image: Some(ResourceSource::Bytes(pgm(&fixtures.normal[k][shot.view as usize - 1]))),
            calibration: Some(ResourceSource::Bytes(serde_json::to_vec(&json!({"schemaVersion": 1, "mmPerPx": 0.125,
                "source": "synthetic exact scale", "camera": shot.camera, "view": shot.view, "size": SIM_SIZE})).unwrap())) }).collect(),
    }).unwrap();
    let publish_ms = milliseconds(published_at);
    let loaded_at = Instant::now();
    let prepared = Prepared::load(bundle.clone(), engine, &recipe).unwrap();
    let prepared_load_ms = milliseconds(loaded_at);
    let warming = Instant::now();
    prepared.warm().unwrap();
    let warm_ms = milliseconds(warming);
    let mut wall = Vec::with_capacity(parts * 4);
    let mut engine_times = Vec::with_capacity(parts * 4);
    let mut part_times = Vec::with_capacity(parts);
    let mut tails = Vec::with_capacity(parts);
    let steady_evidence_path = root.join("steady-records.jsonl");
    let steady_evidence_file = std::fs::OpenOptions::new().write(true).create_new(true).open(&steady_evidence_path).unwrap();
    let mut evidence = BufWriter::new(steady_evidence_file);
    let steady_at = Instant::now();
    let mut samples = vec![memory_sample(0, steady_at)];
    for part in 0..parts {
        let mut record = measured_part(
            &prepared,
            &fixtures.normal,
            &format!("p0-{}v-{}-{part}", view_count, std::process::id()),
        );
        record["part"] = json!(part + 1);
        assert_judgement(&record, Verdict::Ok, 0);
        assert_eq!(record["measurementErrors"], 0);
        for call in record["calls"].as_array().unwrap() {
            wall.push(call["preparedMeasureMs"].as_f64().unwrap());
            engine_times.push(call["engineRunIoParseMs"].as_f64().unwrap());
            assert!(call["coverage"].as_f64().unwrap() >= 0.95);
        }
        part_times.push(record["serialPartMeasureAndJudgeMs"].as_f64().unwrap());
        tails.push(record["lastSelectedFrameToJudgeMs"].as_f64().unwrap());
        serde_json::to_writer(&mut evidence, &record).unwrap();
        evidence.write_all(b"\n").unwrap();
        drop(record);
        if (part + 1) % 10 == 0 || part + 1 == parts {
            samples.push(memory_sample(part + 1, steady_at));
        }
    }
    let steady_total_ms = milliseconds(steady_at);
    evidence.flush().unwrap();
    evidence.get_ref().sync_all().unwrap();
    drop(evidence);
    let gap = measured_part(
        &prepared,
        &fixtures.gap,
        &format!("p0-{}v-{}-gap", view_count, std::process::id()),
    );
    assert_judgement(&gap, Verdict::NgGap, 0);
    assert_eq!(gap["measurementErrors"], 0);
    assert!(gap["calls"][1]["gapStations"].as_u64().unwrap() > 6);
    let empty = measured_part(
        &prepared,
        &fixtures.empty,
        &format!("p0-{}v-{}-empty", view_count, std::process::id()),
    );
    assert_judgement(&empty, Verdict::ErrInspect, judge::fault::PROCESS_TIMEOUT);
    assert_eq!(empty["measurementErrors"], 4);
    assert!(empty["calls"]
        .as_array()
        .unwrap()
        .iter()
        .all(|call| call["error"]
            .as_str()
            .is_some_and(|e| e.contains("检测区内没找到胶"))));
    prepared.verify().unwrap();
    let records: Vec<Value> = std::fs::read_to_string(&steady_evidence_path).unwrap().lines()
        .map(|line| serde_json::from_str(line).unwrap()).collect();
    assert_eq!(records.len(), parts);
    let resources: Vec<_> = bundle
        .manifest
        .files
        .iter()
        .map(|file| artifact(&bundle.root.join(&file.path)))
        .collect();
    let result = json!({"mode": if view_count == 1 { "single_device_single_view" } else { "single_device_three_views" },
        "physicalDevices": 1, "viewCount": view_count, "shotViews": recipe.shots.iter().map(|s| s.view).collect::<Vec<_>>(),
        "shotsPerPart": 4, "deviceFramesPerPart": 4, "viewImagesPerPart": 4 * view_count as usize,
        "selectedMeasurementsPerPart": 4, "steadyParts": parts, "steadyMeasurements": wall.len(),
        "warmupMeasurements": 4, "scenarioProbeMeasurements": 8, "imageSize": SIM_SIZE,
        "recipeRevision": recipe.revision_id, "bundleId": bundle.id, "bundleRoot": bundle.root,
        "releaseManifest": artifact(&bundle.root.join("manifest.json")), "releaseResources": resources,
        "fixtures": artifact(&fixture_manifest), "fixtureManifest": fixtures.manifest,
        "fixtureRenderMs": fixture_render_ms, "publishMs": publish_ms, "preparedLoadMs": prepared_load_ms,
        "warmupMs": warm_ms, "enginePreviouslyUsedInProcess": engine_already_used,
        "warmupKind": if engine_already_used { "already_used_engine" } else { "first_graph_use_for_loaded_engine" },
        "steadyTotalMs": steady_total_ms, "preparedMeasureMs": distribution(&wall),
        "engineRunIoParseMs": distribution(&engine_times), "serialPartMeasureAndJudgeMs": distribution(&part_times),
        "lastSelectedFrameToJudgeMs": distribution(&tails), "productionQueueWaitMs": null, "productionPartEndToJudgeMs": null,
        "steadyEvidence": artifact(&steady_evidence_path),
        "steadyLoopTiming": "Includes evidence serialization, buffered JSONL writes and memory sampling outside each per-part timer; final flush/fsync is also outside the per-part and steady-loop timers.",
        "memory": memory_trend(&samples), "steadyRecords": records,
        "scenarioProbes": {"partialGap": gap, "wholeEmpty": empty}, "passed": true});
    println!("P0 {view_count}V: {parts} parts / {} measurements; Prepared p50/p95/max {:.3}/{:.3}/{:.3} ms; warmup {:.3} ms; bundle {}",
        wall.len(), result["preparedMeasureMs"]["p50"].as_f64().unwrap(), result["preparedMeasureMs"]["p95"].as_f64().unwrap(),
        result["preparedMeasureMs"]["max"].as_f64().unwrap(), warm_ms, bundle.id);
    result
}

fn command_output(command: &str, args: &[&str]) -> Option<String> {
    let output = Command::new(command).args(args).output().ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
}

#[test]
#[ignore = "real taught-path DLL, full-resolution 100-part/400-measurement regression per mode; set LYFLOW_CORE_DLL and GLUESIGHT_P0_REPORT_DIR"]
fn native_full_resolution_frozen_bundle_single_and_tricam_regression() {
    let parts = std::env::var("GLUESIGHT_P0_PARTS")
        .map(|s| {
            s.parse::<usize>()
                .expect("GLUESIGHT_P0_PARTS must be an integer")
        })
        .unwrap_or(100);
    assert!(
        parts >= 100,
        "P0 regression requires at least 100 parts / 400 stable measurements per mode"
    );
    let root = std::env::var_os("GLUESIGHT_P0_REPORT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            std::env::temp_dir().join(format!(
                "gluesight-p0-regression-{}-{}",
                std::process::id(),
                ly_plc::now_ms()
            ))
        });
    std::fs::create_dir_all(&root).unwrap();
    let root = root.canonicalize().unwrap();
    assert!(
        !root.join("report.json").exists(),
        "Use a fresh report directory to preserve previous evidence"
    );
    let dll = PathBuf::from(
        std::env::var_os("LYFLOW_CORE_DLL")
            .expect("Set LYFLOW_CORE_DLL to the real taught-path DLL"),
    );
    let source = Path::new(env!("CARGO_MANIFEST_DIR"));
    let fixture_style = std::env::var("GLUESIGHT_P0_FIXTURE_STYLE").unwrap_or_else(|_| "simulated_metal".into());
    assert!(matches!(fixture_style.as_str(), "simulated_metal" | "clean_step_edge"), "Unknown fixture style");
    let clean = fixture_style == "clean_step_edge";
    let initial_memory = memory();
    let dll_at = Instant::now();
    let engine = Arc::new(Engine::load(&dll).unwrap());
    let dll_load_ms = milliseconds(dll_at);
    let dll_artifact = artifact(&dll);
    let run_at = Instant::now();
    let started_at = ly_plc::now_ms();
    let cases = vec![
        run_case(&root, engine.clone(), 1, parts, false, clean),
        run_case(&root, engine.clone(), 3, parts, true, clean),
    ];
    let report = json!({"schemaVersion": 1, "passed": true, "fixtureStyle": fixture_style, "startedAtUnixMs": started_at,
        "elapsedMs": milliseconds(run_at), "pid": std::process::id(), "os": std::env::consts::OS, "arch": std::env::consts::ARCH,
        "software": {"package": env!("CARGO_PKG_NAME"), "version": env!("CARGO_PKG_VERSION"),
            "graphVersion": GRAPH_VERSION, "gitCommit": command_output("git", &["-C", source.to_str().unwrap(), "rev-parse", "HEAD"]),
            "gitStatus": command_output("git", &["-C", source.to_str().unwrap(), "status", "--porcelain"]),
            "rustc": command_output("rustc", &["--version"]),
            "originalSourceDirectory": source, "originalTestExecutable": std::env::current_exe().unwrap(),
            "snapshotPolicy": "Software source and test executable are byte-for-byte archived under this fresh report directory so later compilation cannot invalidate referenced evidence.",
            "testExecutable": software_snapshot(&std::env::current_exe().unwrap(), &root.join("software/test-executable.exe")),
            "testArguments": std::env::args().skip(1).collect::<Vec<_>>(),
            "sourceFiles": (["Cargo.toml", "Cargo.lock", "src/production.rs", "src/production/regression.rs", "src/vision.rs", "src/vision/taught.rs", "src/recipe.rs", "src/simimage.rs", "src/sim.rs", "src/judge.rs", "src/release.rs", "src/frame.rs", "src/measure.rs"]
                .iter().map(|p| software_snapshot(&source.join(p), &root.join("software/src-tauri").join(p))).collect::<Vec<_>>()),
            "reportVerifier": software_snapshot(&source.parent().unwrap().join("scripts/p0-regression-report.py"), &root.join("software/scripts/p0-regression-report.py"))},
        "engine": {"version": engine.version, "identity": engine.identity, "dll": dll_artifact, "loadAndSelfCheckMs": dll_load_ms},
        "initialMemory": initial_memory, "finalMemory": memory(), "cases": cases,
        "scope": {"execution": "Serial direct production::Prepared calls using a verified immutable release bundle and real DLL, followed by the local judge.",
            "engineTiming": "engineRunIoParseMs is ShotMeasurement.ms: native run, injected image I/O and output parsing, integer milliseconds. preparedMeasureMs includes graph contract validation with high-resolution wall time.",
            "tailTiming": "lastSelectedFrameToJudgeMs starts immediately before submitting the fourth already-rendered selected image and ends after local judgement. serialPartMeasureAndJudgeMs spans all four serial measurements, wrapper assertions/telemetry and judgement. Both use the same captured judgement completion time. Neither is a production partEnd/queue tail measurement.",
            "warmupTiming": "loadAndSelfCheckMs measures DLL load; warmupMs includes four full-size frozen-image measures and bundle verification. Only the first case is the first graph use of this benchmark's freshly loaded Engine. Run this exact ignored test alone with --test-threads=1 to isolate process-cold native initialization; other test activity and OS/DLL file caches are not controlled here.",
            "unknownMetrics": ["productionQueueWaitMs", "productionPartEndToJudgeMs", "physical three-view SDK synchronization", "field accuracy", "absence of memory leaks beyond this finite run"],
            "excluded": ["production dispatcher queue", "PLC handshake and acknowledgement", "camera acquisition and transport", "physical tricam SDK unpacking/trigger wiring", "Recorder write latency", "end-to-end production scheduling proof"],
            "artifacts": "Artifacts are identified by explicit report paths and byte counts; recipe revisions and bundle IDs are explicit business references."}});
    write_json(&root.join("report.json"), &report);
    println!(
        "P0 regression evidence: {}",
        root.join("report.json").display()
    );
}
