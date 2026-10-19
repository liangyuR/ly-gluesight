use super::line::{Layout, Line, PartOrder, Timing};
use super::s7_server::S7Server;
use super::*;
use crate::plc_plan::{PlanShot, PlcPlan};
use crate::plc_session::{PlcSession, SessionEvent, SessionPhase};
use ly_plc::{EventSink, Logbook, PlcEngine};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

fn pct(mut v: Vec<f64>, label: &str) {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let q = |p: f64| v[((v.len() as f64 - 1.0) * p).round() as usize];
    let mean = v.iter().sum::<f64>() / v.len() as f64;
    println!("{label:>10}: n={} min={:.1} p50={:.1} p90={:.1} p99={:.1} max={:.1} mean={:.1}", v.len(), v[0], q(0.5), q(0.9), q(0.99), v[v.len() - 1], mean);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
#[ignore = "布防耗时测量：cargo test --release --lib arm_latency -- --ignored --nocapture"]
async fn arm_latency_distribution_over_virtual_s7() {
    let parts: usize = std::env::var("ARM_BENCH_PARTS").ok().and_then(|v| v.parse().ok()).unwrap_or(200);
    let mut config = crate::inspection::default_plc_config();
    let layout = Layout::from_config(&config).unwrap();
    let memory = layout.memory();
    let server = S7Server::start(memory.clone(), Some((layout.heartbeat(), Duration::from_millis(500)))).await.unwrap();
    config.connection.host = "127.0.0.1".into();
    config.connection.port = server.port();
    let directory = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/arm-bench")
        .join(format!("{}-{}", std::process::id(), SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()));
    std::fs::create_dir_all(&directory).unwrap();
    let sink: EventSink = Arc::new(|_| {});
    let engine = PlcEngine::new(config, Arc::new(Logbook::open(&directory.join("plc.db"), sink.clone(), 1).unwrap()), sink);
    engine.connect().await;
    let line = Line::new(memory, layout);
    let mut session = PlcSession::open(directory.join("session.json"));
    let plan = PlcPlan::compile("part-A".into(), 7, vec!["cam1".into()], (1..=12).map(|i| PlanShot {
        shot_id: format!("P{i}"), pose_id: format!("P{i}"), camera_ids: vec!["cam1".into()],
    }).collect()).unwrap();
    let timing = Timing {
        poll: Duration::from_millis(10), shot_interval: Duration::from_millis(15), end_delay: Duration::from_millis(20),
        ack_delay: Duration::from_millis(20), settle: Duration::from_millis(300), arm_timeout: Duration::from_secs(5),
        result_timeout: Duration::from_secs(10), ready_timeout: Duration::from_secs(10), release_timeout: Duration::from_secs(10),
    };
    let order = PartOrder { sn: "BENCH-1".into(), product_code: 3, plan_version: plan.plan_version, shot_count: plan.shot_count, device_shots: plan.device_shots };
    let abort = AtomicBool::new(false);
    let finished = AtomicBool::new(false);
    let (mut totals, mut outputs) = (Vec::new(), Vec::new());
    let (mut writes, mut confirms, mut journals, mut polls) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    let mut failures = 0;
    let plc = async {
        for n in 0..parts {
            if let Err(error) = line.ensure_ready(&timing, &abort).await { println!("ensure_ready {n}: {error}"); }
            let order = PartOrder { sn: format!("BENCH-{n}"), ..order.clone() };
            if let Err(error) = line.run_part(&timing, &order, Scenario::Normal, |_, _| {}, &abort).await { println!("part {n}: {error}"); }
        }
        finished.store(true, Ordering::SeqCst);
    };
    let pc = async {
        let mut seed = 12345u64;
        while !finished.load(Ordering::SeqCst) {
            match session.poll_gated(&engine, false, true, true, true).await {
                SessionEvent::Start(_) => {
                    seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                    tokio::time::sleep(Duration::from_millis(20 + (seed >> 33) % 50)).await;
                    let started = Instant::now();
                    let result = match session.arm_outputs(&engine, &plan).await {
                        Ok(()) => { outputs.push(started.elapsed().as_secs_f64() * 1000.0); session.commit_arm(&engine).await }
                        Err(error) => Err(error),
                    };
                    let total = started.elapsed().as_secs_f64() * 1000.0;
                    if let Some(t) = session.arm_timing() {
                        writes.push(t.write_ms); confirms.push(t.confirm_ms); polls.push(t.confirm_polls as f64);
                        if let Some(j) = t.journal_ms { journals.push(j); }
                    }
                    match result { Ok(()) => totals.push(total), Err(error) => { failures += 1; println!("arm failed: {error}"); } }
                }
                SessionEvent::End(_) => { let _ = session.report(&engine, 1, 0).await; }
                _ => {}
            }
            if session.phase() == SessionPhase::Fault { tokio::time::sleep(Duration::from_millis(5)).await; }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    };
    tokio::join!(plc, pc);
    println!("parts={parts} failures={failures} load-sample={:?}", std::env::var("ARM_BENCH_NOTE").ok());
    pct(outputs, "outputs");
    pct(totals, "+journal");
    pct(writes, "write");
    pct(confirms, "confirm");
    pct(polls, "polls");
    pct(journals, "journal");
    assert_eq!(failures, 0, "布防失败");
    engine.disconnect().await;
    drop(server);
}
