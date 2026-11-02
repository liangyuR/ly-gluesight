//! 相机事件探测（现场工具，不是测试）：在 Line0 高电平门控下，看相机能给出哪些“采集开始 / 结束”信号、它们与帧的先后和延迟，
//! 为“判定一件的帧已收完、取代固定收尾窗口”定方案。现场 JHEM131GC-XS 没有 GenICam 事件，主要靠轮询 Line0 电平。
//!
//! 做的事：
//! 1. 按 cameras.json（或 `--serials`）打开相机，按生产同一套规则配置门控、帧率、频闪与 Chunk（`--keep` 则不改相机当前设置）；
//! 2. 列出每台相机 `EventSelector` 支持的事件并打开（`--events` 可只开指定几个；相机没有事件时跳过）；
//! 3. 每台相机一个线程按 `--poll-ms`（默认 5 ms）轮询 `LineStatusAll` 读 Line0 电平；电平变了即记一次上升 / 下降沿，同时锁存
//!    相机时钟（`GevTimestampControlLatch`）并读 Line0 跳变计数器（`Counter0`，计数来源设为 Line0，用来发现轮询漏掉的短脉冲）；
//! 4. 开流后按时间顺序打印每个边沿、事件与每一帧：PC 收到 / 看到的时刻、相机时间戳、帧号；
//! 5. 结束（到时或按回车）后写 CSV，并按上升沿—下降沿配对出每次门控，统计首帧比上升沿晚多少、最后一帧比看到下降沿晚多少、
//!    下降沿之后又来了几帧、按相机时钟门控里应有几帧；另统计轮询一次的耗时。
//!
//! 输出两个文件：`camera-events-<时间>.log`（屏幕上打印的全部内容，含出错信息）与 `camera-events-<时间>.csv`（每条事件与帧），
//! 默认放在桌面（没有桌面时放当前目录），`--dir` 可另指目录。
//!
//! 现场机：把 release 版 camera_event_probe.exe 复制到 GlueSight 安装目录（与 GlueSight 主程序同一目录，程序从这里的
//! runtime\mvs 加载相机 SDK），关掉 GlueSight 与 MVS 客户端（相机独占打开），在命令行窗口运行：
//!
//! ```powershell
//! & "<安装目录>\camera_event_probe.exe" --config "<项目目录>\config\cameras.json" --seconds 300
//! ```
//!
//! 开发机（装了 MVS 客户端）也可在仓库根目录直接跑：
//! `cargo run --manifest-path src-tauri/Cargo.toml --example camera_event_probe -- --config <cameras.json>`。
//!
//! 参数：`--config <cameras.json>` 或 `--serials 序列号1,序列号2,序列号3`（按默认的 Line0 高电平、10 fps 配置）；
//! `--seconds N` 最长运行秒数（默认 300，按回车提前结束）；`--events 名1,名2` 只开这几个事件（默认全部）；
//! `--keep` 不改相机设置；`--quiet` 屏幕上只打印边沿与事件（日志文件里仍有每一帧）；`--dir <目录>` 输出目录；
//! `--poll-ms N` 轮询 Line0 的间隔（默认 5，0 = 不轮询）。

use std::collections::BTreeMap;
use std::ffi::c_void;
use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use gluesight_lib::probe_api::{
    configure_for_probe, CameraConfig, MvsDevice, MvsEventInfo, MvsFrameInfo,
};

static LOG: OnceLock<Mutex<std::fs::File>> = OnceLock::new();

/// 打印一行，同时写进日志文件。
macro_rules! say {
    ($($arg:tt)*) => {{
        let line = format!($($arg)*);
        println!("{line}");
        log_line(&line);
    }};
}

fn log_line(line: &str) {
    if let Some(file) = LOG.get() {
        let _ = writeln!(file.lock().unwrap(), "{line}");
    }
}

enum Kind {
    Frame {
        frame_num: u32,
        trigger_index: u32,
        lost: u32,
    },
    Event {
        name: String,
        block: u64,
    },
    /// 轮询看到 Line0 电平变化：`window_us` 为上一次读到旧电平到这次读到新电平的间隔（边沿就在这段里）
    Line {
        rising: bool,
        counter: Option<i64>,
        window_us: u64,
    },
}

struct Record {
    cam: usize,
    host_us: u64,
    dev_ts: u64,
    kind: Kind,
}

/// 回调的 user 指针指向它：哪台相机、计时起点、往主线程送记录的通道。
struct Sink {
    cam: usize,
    start: Instant,
    tx: Mutex<mpsc::Sender<Record>>,
}

impl Sink {
    fn send(&self, dev_ts: u64, kind: Kind) {
        self.send_at(self.start.elapsed().as_micros() as u64, dev_ts, kind);
    }

    fn send_at(&self, host_us: u64, dev_ts: u64, kind: Kind) {
        let _ = self.tx.lock().unwrap().send(Record {
            cam: self.cam,
            host_us,
            dev_ts,
            kind,
        });
    }
}

extern "system" fn on_image(_data: *mut u8, info: *mut MvsFrameInfo, user: *mut c_void) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if info.is_null() || user.is_null() {
            return;
        }
        let (info, sink) = unsafe { (&*info, &*(user as *const Sink)) };
        let dev_ts = (u64::from(info.dev_ts_high) << 32) | u64::from(info.dev_ts_low);
        sink.send(
            dev_ts,
            Kind::Frame {
                frame_num: info.frame_num,
                trigger_index: info.trigger_index,
                lost: info.lost_packet,
            },
        );
    }));
}

extern "system" fn on_event(info: *mut MvsEventInfo, user: *mut c_void) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if info.is_null() || user.is_null() {
            return;
        }
        let (info, sink) = unsafe { (&*info, &*(user as *const Sink)) };
        sink.send(
            info.timestamp(),
            Kind::Event {
                name: info.event_name(),
                block: info.block_id(),
            },
        );
    }));
}

extern "system" fn on_exception(msg: u32, user: *mut c_void) {
    let cam = if user.is_null() {
        usize::MAX
    } else {
        unsafe { &*(user as *const Sink) }.cam
    };
    say!("!! 相机 {cam} 异常 0x{msg:08X}（0x8001 = 断开）");
}

struct Args {
    cameras: Vec<CameraConfig>,
    seconds: u64,
    events: Option<Vec<String>>,
    keep: bool,
    quiet: bool,
    dir: std::path::PathBuf,
    poll_ms: u64,
}

fn usage() -> ! {
    eprintln!(
        "{}",
        include_str!("camera_event_probe.rs")
            .lines()
            .take_while(|l| l.starts_with("//!"))
            .map(|l| l.trim_start_matches("//!").trim_start())
            .collect::<Vec<_>>()
            .join("\n")
    );
    std::process::exit(2);
}

fn parse() -> Args {
    let mut it = std::env::args().skip(1);
    let (mut config, mut serials, mut seconds, mut events, mut keep, mut quiet, mut dir) =
        (None, None, 300, None, false, false, None);
    let mut poll_ms = 5;
    while let Some(arg) = it.next() {
        let mut value = || it.next().unwrap_or_else(|| usage());
        match arg.as_str() {
            "--config" => config = Some(value()),
            "--serials" => serials = Some(value()),
            "--seconds" => seconds = value().parse().unwrap_or_else(|_| usage()),
            "--events" => {
                events = Some(
                    value()
                        .split(',')
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                        .collect(),
                )
            }
            "--keep" => keep = true,
            "--quiet" => quiet = true,
            "--dir" => dir = Some(value()),
            "--poll-ms" => poll_ms = value().parse().unwrap_or_else(|_| usage()),
            _ => usage(),
        }
    }
    let cameras: Vec<CameraConfig> = match (config, serials) {
        (Some(path), None) => {
            let text =
                std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("读不了 {path}：{e}"));
            let value: serde_json::Value =
                serde_json::from_str(&text).unwrap_or_else(|e| panic!("{path} 不是 JSON：{e}"));
            serde_json::from_value::<Vec<CameraConfig>>(value["cameras"].clone())
                .unwrap_or_else(|e| panic!("{path} 里没有可读的 cameras：{e}"))
        }
        (None, Some(list)) => list
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .enumerate()
            .map(|(i, serial)| CameraConfig {
                id: format!("cam{}", i + 1),
                name: format!("相机 {}", i + 1),
                serial: serial.into(),
                ..Default::default()
            })
            .collect(),
        _ => usage(),
    };
    let cameras: Vec<CameraConfig> = cameras
        .into_iter()
        .filter(|c| !c.serial.trim().is_empty())
        .collect();
    if cameras.is_empty() {
        eprintln!("没有绑定序列号的相机");
        std::process::exit(2);
    }
    let dir = dir.map(std::path::PathBuf::from).unwrap_or_else(|| {
        std::env::var_os("USERPROFILE")
            .map(|home| std::path::PathBuf::from(home).join("Desktop"))
            .filter(|desktop| desktop.is_dir())
            .unwrap_or_else(|| std::path::PathBuf::from("."))
    });
    Args {
        cameras,
        seconds,
        events,
        keep,
        quiet,
        dir,
        poll_ms,
    }
}

/// 轮询一台相机的统计：读电平的次数、耗时与失败。
#[derive(Default)]
struct PollStats {
    reads: u64,
    total_us: u64,
    max_us: u64,
    errors: u64,
    last_error: Option<String>,
}

/// 锁存并读出相机当前时钟（计数值，频率见 GevTimestampTickFrequency）；读不了为 0。
fn latch_clock(device: &MvsDevice) -> u64 {
    device
        .command("GevTimestampControlLatch")
        .and_then(|()| device.get_int("GevTimestampValue"))
        .map_or(0, |v| v.max(0) as u64)
}

/// 把 Line0 跳变计数器（Counter0）设为数 Line0 的跳变并清零；相机不支持时返回原因。
fn arm_counter(device: &MvsDevice) -> Result<(), String> {
    device.set_enum("CounterSelector", "Counter0")?;
    device.set_enum("CounterEventSource", "Line0")?;
    device.command("CounterReset")
}

/// 每 `poll` 读一次 Line0 电平（`LineStatusAll` 第 0 位），电平变了就记一个边沿；直到 `stop`。
fn poll_line(
    device: Arc<MvsDevice>,
    sink: Arc<Sink>,
    counter: bool,
    poll: Duration,
    stop: Arc<AtomicBool>,
) -> PollStats {
    let mut stats = PollStats::default();
    let (mut level, mut seen) = (None::<bool>, sink.start.elapsed());
    while !stop.load(Ordering::SeqCst) {
        let before = sink.start.elapsed();
        let read = device.get_int("LineStatusAll");
        let spent = (sink.start.elapsed() - before).as_micros() as u64;
        stats.reads += 1;
        stats.total_us += spent;
        stats.max_us = stats.max_us.max(spent);
        match read {
            Ok(bits) => {
                let now = bits & 1 == 1;
                if level.is_some_and(|old| old != now) {
                    let dev = latch_clock(&device);
                    let count = counter
                        .then(|| device.get_int("CounterCurrentValue").ok())
                        .flatten();
                    sink.send_at(
                        before.as_micros() as u64,
                        dev,
                        Kind::Line {
                            rising: now,
                            counter: count,
                            window_us: (before - seen).as_micros() as u64,
                        },
                    );
                }
                level = Some(now);
                seen = before;
            }
            Err(e) => {
                stats.errors += 1;
                stats.last_error = Some(e);
            }
        }
        std::thread::sleep(poll);
    }
    stats
}

/// 结束类事件：一次门控 / 采集 / 连拍结束或 Line0 下降沿（不含逐帧的曝光、出帧结束）。
fn is_end_event(name: &str) -> bool {
    (name.ends_with("End") && !matches!(name, "ExposureEnd" | "FrameEnd"))
        || name.contains("FallingEdge")
}

fn percentile(values: &mut [f64], p: f64) -> f64 {
    values.sort_by(f64::total_cmp);
    values[((values.len() - 1) as f64 * p).round() as usize]
}

/// 本地时间的“年月日-时分秒”，用于输出文件名。
fn chrono_stamp() -> String {
    chrono::Local::now().format("%Y%m%d-%H%M%S").to_string()
}

fn main() {
    let args = parse();
    let stamp = chrono_stamp();
    let base = args.dir.join(format!("camera-events-{stamp}"));
    let (log_path, csv_path) = (base.with_extension("log"), base.with_extension("csv"));
    match std::fs::File::create(&log_path) {
        Ok(file) => {
            let _ = LOG.set(Mutex::new(file));
        }
        Err(e) => eprintln!(
            "建不了日志文件 {}：{e}（只在屏幕上打印）",
            log_path.display()
        ),
    }
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        log_line(&format!("!! 出错退出：{info}"));
        default_hook(info);
    }));
    say!("日志 {}，明细 {}", log_path.display(), csv_path.display());
    let start = Instant::now();
    let (tx, rx) = mpsc::channel();
    let sinks: Vec<Arc<Sink>> = (0..args.cameras.len())
        .map(|cam| {
            Arc::new(Sink {
                cam,
                start,
                tx: Mutex::new(tx.clone()),
            })
        })
        .collect();
    drop(tx);
    let mut devices: Vec<Arc<MvsDevice>> = Vec::new();
    let mut counters: Vec<bool> = Vec::new();
    let mut ticks: Vec<Option<f64>> = Vec::new();
    for (cam, config) in args.cameras.iter().enumerate() {
        let device = MvsDevice::open(&config.serial)
            .unwrap_or_else(|e| panic!("打开相机 {}（{}）失败：{e}", config.id, config.serial));
        say!(
            "== 相机 {cam} {} · {} · {} · {}",
            config.id,
            config.name,
            device.summary.model,
            device.summary.serial
        );
        if !args.keep {
            match configure_for_probe(&device, config) {
                Ok(warnings) => warnings.iter().for_each(|w| say!("   配置提示：{w}")),
                Err(e) => panic!(
                    "按生产规则配置相机 {} 失败：{e}（可加 --keep 用相机当前设置）",
                    config.id
                ),
            }
        }
        for node in [
            "TriggerSelector",
            "TriggerMode",
            "TriggerSource",
            "TriggerActivation",
        ] {
            say!(
                "   {node} = {}",
                device
                    .get_enum(node)
                    .unwrap_or_else(|e| format!("读不了：{e}"))
            );
        }
        say!(
            "   AcquisitionFrameRate = {}",
            device
                .get_float("AcquisitionFrameRate")
                .map(|v| v.to_string())
                .unwrap_or_else(|e| format!("读不了：{e}"))
        );
        let tick = device
            .get_int("GevTimestampTickFrequency")
            .ok()
            .filter(|&f| f > 0)
            .map(|f| f as f64);
        say!(
            "   相机时间戳频率 = {}",
            tick.map(|f| format!("{f} Hz"))
                .unwrap_or("读不出（时间戳按原始计数打印）".into())
        );
        ticks.push(tick);
        let supported = device.enum_entries("EventSelector").unwrap_or_else(|e| {
            say!("   读不出 EventSelector：{e}");
            Vec::new()
        });
        say!(
            "   支持的事件（{}）：{}",
            supported.len(),
            supported.join("、")
        );
        let counter = match arm_counter(&device) {
            Ok(()) => {
                say!("   Line0 跳变计数器 Counter0 已清零");
                true
            }
            Err(e) => {
                say!("   Line0 跳变计数器不可用：{e}");
                false
            }
        };
        counters.push(counter);
        say!(
            "   Line0 当前电平 = {}，锁存相机时钟 = {}",
            device
                .get_int("LineStatusAll")
                .map(|v| (v & 1).to_string())
                .unwrap_or_else(|e| format!("读不了：{e}")),
            match latch_clock(&device) {
                0 => "读不了".to_string(),
                v => format!("{v}"),
            }
        );
        let user = Arc::as_ptr(&sinks[cam]) as *mut c_void;
        for name in supported
            .iter()
            .filter(|n| args.events.as_ref().is_none_or(|wanted| wanted.contains(n)))
        {
            match device.on_event(name, on_event, user) {
                Ok(()) => say!("   已打开 {name}"),
                Err(e) => say!("   打不开 {name}：{e}"),
            }
        }
        device
            .start(on_image, on_exception, user)
            .unwrap_or_else(|e| panic!("相机 {} 开流失败：{e}", config.id));
        devices.push(Arc::new(device));
    }

    let stop = Arc::new(AtomicBool::new(false));
    let pollers: Vec<_> = if args.poll_ms == 0 {
        Vec::new()
    } else {
        devices
            .iter()
            .enumerate()
            .map(|(cam, device)| {
                let (device, sink, stop) = (device.clone(), sinks[cam].clone(), stop.clone());
                let (counter, poll) = (counters[cam], Duration::from_millis(args.poll_ms));
                std::thread::spawn(move || poll_line(device, sink, counter, poll, stop))
            })
            .collect()
    };
    {
        let stop = stop.clone();
        std::thread::spawn(move || {
            let _ = std::io::stdin().read_line(&mut String::new());
            stop.store(true, Ordering::SeqCst);
        });
    }
    say!(
        "\n开始记录，最长 {} 秒；按回车提前结束。正常节拍跑几件，段间有拍照信号开关的件最好。\n",
        args.seconds
    );
    let ms = |cam: usize, ts: u64| {
        ticks[cam].map_or(format!("{ts}"), |f| {
            format!("{:.3}ms", ts as f64 * 1000.0 / f)
        })
    };
    let deadline = start + Duration::from_secs(args.seconds);
    let mut records: Vec<Record> = Vec::new();
    while !stop.load(Ordering::SeqCst) && Instant::now() < deadline {
        let Ok(record) = rx.recv_timeout(Duration::from_millis(200)) else {
            continue;
        };
        let t = record.host_us as f64 / 1e6;
        let id = &args.cameras[record.cam].id;
        match &record.kind {
            Kind::Event { name, block } => say!("{t:>10.3}s {id:<6} 事件 {name:<22} 帧块 {block:<8} 相机时刻 {}", ms(record.cam, record.dev_ts)),
            Kind::Frame { frame_num, trigger_index, lost } if !args.quiet => say!(
                "{t:>10.3}s {id:<6} 帧   #{frame_num:<20} 触发计数 {trigger_index:<4} 丢包 {lost:<3} 相机时刻 {}",
                ms(record.cam, record.dev_ts)
            ),
            Kind::Line { rising, counter, window_us } => say!(
                "{t:>10.3}s {id:<6} Line0 {} 观察窗 {:.1}ms 计数 {} 相机时刻 {}",
                if *rising { "↑ 上升" } else { "↓ 下降" },
                *window_us as f64 / 1000.0,
                counter.map_or("—".to_string(), |c| c.to_string()),
                ms(record.cam, record.dev_ts)
            ),
            Kind::Frame { .. } => {}
        }
        records.push(record);
    }
    stop.store(true, Ordering::SeqCst);
    let stats: Vec<PollStats> = pollers
        .into_iter()
        .map(|h| h.join().unwrap_or_default())
        .collect();
    let counter_end: Vec<Option<i64>> = devices
        .iter()
        .zip(&counters)
        .map(|(d, &on)| on.then(|| d.get_int("CounterCurrentValue").ok()).flatten())
        .collect();
    while let Ok(record) = rx.try_recv() {
        records.push(record);
    }
    drop(devices);
    drop(sinks);

    let mut csv = String::from(
        "camera,host_ms,kind,name,frame_block_or_counter,trigger_index,lost_packets,device_ts,device_ms,poll_window_ms\n",
    );
    for r in &records {
        let dev_ms = ticks[r.cam].map_or(String::new(), |f| {
            format!("{:.3}", r.dev_ts as f64 * 1000.0 / f)
        });
        let host = r.host_us as f64 / 1000.0;
        let id = &args.cameras[r.cam].id;
        let line = match &r.kind {
            Kind::Frame {
                frame_num,
                trigger_index,
                lost,
            } => format!(
                "{id},{host:.3},frame,,{frame_num},{trigger_index},{lost},{},{dev_ms},\n",
                r.dev_ts
            ),
            Kind::Event { name, block } => format!(
                "{id},{host:.3},event,{name},{block},,,{},{dev_ms},\n",
                r.dev_ts
            ),
            Kind::Line {
                rising,
                counter,
                window_us,
            } => format!(
                "{id},{host:.3},line0,{},{},,,{},{dev_ms},{:.3}\n",
                if *rising { "rise" } else { "fall" },
                counter.map_or(String::new(), |c| c.to_string()),
                r.dev_ts,
                *window_us as f64 / 1000.0
            ),
        };
        csv.push_str(&line);
    }
    match std::fs::File::create(&csv_path).and_then(|mut f| f.write_all(csv.as_bytes())) {
        Ok(()) => say!("\n已写 {}（{} 条）", csv_path.display(), records.len()),
        Err(e) => say!("\n写 {} 失败：{e}", csv_path.display()),
    }

    say!("\n==== 汇总 ====");
    for (cam, config) in args.cameras.iter().enumerate() {
        let period_ms = 1000.0 / f64::from(config.frame_rate.max(0.1));
        let gap_ms = (3.0 * period_ms).max(150.0);
        let frames: Vec<(f64, u32, u64)> = records
            .iter()
            .filter(|r| r.cam == cam)
            .filter_map(|r| match r.kind {
                Kind::Frame { frame_num, .. } => {
                    Some((r.host_us as f64 / 1000.0, frame_num, r.dev_ts))
                }
                _ => None,
            })
            .collect();
        let events: Vec<(f64, &str, u64, u64)> = records
            .iter()
            .filter(|r| r.cam == cam)
            .filter_map(|r| match &r.kind {
                Kind::Event { name, block } => {
                    Some((r.host_us as f64 / 1000.0, name.as_str(), *block, r.dev_ts))
                }
                _ => None,
            })
            .collect();
        let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
        events
            .iter()
            .for_each(|e| *counts.entry(e.1).or_default() += 1);
        say!(
            "\n相机 {}：{} 帧；事件 {}",
            config.id,
            frames.len(),
            if counts.is_empty() {
                "无".into()
            } else {
                counts
                    .iter()
                    .map(|(n, c)| format!("{n}×{c}"))
                    .collect::<Vec<_>>()
                    .join("、")
            }
        );
        // 按帧间隔切出门控段：间隔超过 3 个帧周期（至少 150 ms）即一次门控结束
        let mut gates: Vec<(usize, usize)> = Vec::new();
        for (i, f) in frames.iter().enumerate() {
            match gates.last_mut() {
                Some((_, last)) if f.0 - frames[*last].0 <= gap_ms => *last = i,
                _ => gates.push((i, i)),
            }
        }
        say!(
            "  按帧间隔 > {gap_ms:.0} ms 切出 {} 次门控（每次的帧数：{}）",
            gates.len(),
            gates
                .iter()
                .map(|(a, b)| (b - a + 1).to_string())
                .collect::<Vec<_>>()
                .join(" ")
        );
        let ends: Vec<&str> = counts.keys().copied().filter(|n| is_end_event(n)).collect();
        for name in ends {
            let (mut matched, mut delays, mut after_max, mut block_hits, mut dev_gaps) =
                (0, Vec::new(), 0usize, 0, Vec::new());
            for (k, &(first, last)) in gates.iter().enumerate() {
                let (open, close) = (
                    frames[first].0,
                    gates.get(k + 1).map_or(f64::INFINITY, |g| frames[g.0].0),
                );
                let Some(event) = events
                    .iter()
                    .find(|e| e.1 == name && e.0 >= open && e.0 < close)
                else {
                    continue;
                };
                matched += 1;
                let (t_last, num_last, dev_last) = frames[last];
                delays.push(event.0 - t_last);
                after_max = after_max.max(
                    frames[first..=last]
                        .iter()
                        .filter(|f| f.0 > event.0)
                        .count(),
                );
                if event.2 == u64::from(num_last) {
                    block_hits += 1;
                }
                if let Some(f) = ticks[cam] {
                    dev_gaps.push((event.3 as f64 - dev_last as f64) * 1000.0 / f);
                }
            }
            if delays.is_empty() {
                say!("  {name}：{} 次门控里一次也没对上", gates.len());
                continue;
            }
            let (min, p50, max) = (
                delays.iter().copied().fold(f64::INFINITY, f64::min),
                percentile(&mut delays, 0.5),
                delays.iter().copied().fold(f64::NEG_INFINITY, f64::max),
            );
            say!("  {name}：对上 {matched}/{} 次门控；事件到达 − 最后一帧到达 = 最小 {min:.1} / 中位 {p50:.1} / 最大 {max:.1} ms（负数 = 事件先到）；\
                事件之后最多又来 {after_max} 帧；事件帧块号 = 最后一帧帧号 {block_hits}/{matched} 次", gates.len());
            if !dev_gaps.is_empty() {
                let (dmin, dmax) = (
                    dev_gaps.iter().copied().fold(f64::INFINITY, f64::min),
                    dev_gaps.iter().copied().fold(f64::NEG_INFINITY, f64::max),
                );
                say!("    相机时间戳：事件 − 最后一帧 = {dmin:.1} ~ {dmax:.1} ms");
            }
        }
    }
    say!("\n==== Line0 边沿与帧 ====");
    for (cam, config) in args.cameras.iter().enumerate() {
        let period_ms = 1000.0 / f64::from(config.frame_rate.max(0.1));
        let tick = ticks[cam];
        let dev_ms = |ts: u64| tick.map(|f| ts as f64 * 1000.0 / f);
        let frames: Vec<(f64, u64)> = records
            .iter()
            .filter(|r| r.cam == cam && matches!(r.kind, Kind::Frame { .. }))
            .map(|r| (r.host_us as f64 / 1000.0, r.dev_ts))
            .collect();
        let edges: Vec<(f64, bool, u64, f64)> = records
            .iter()
            .filter(|r| r.cam == cam)
            .filter_map(|r| match r.kind {
                Kind::Line {
                    rising, window_us, ..
                } => Some((
                    r.host_us as f64 / 1000.0,
                    rising,
                    r.dev_ts,
                    window_us as f64 / 1000.0,
                )),
                _ => None,
            })
            .collect();
        let s = &stats.get(cam);
        say!(
            "\n相机 {}：上升沿 {} 次、下降沿 {} 次；轮询 {} 次，单次读电平平均 {:.2} ms、最长 {:.2} ms，失败 {} 次{}",
            config.id,
            edges.iter().filter(|e| e.1).count(),
            edges.iter().filter(|e| !e.1).count(),
            s.map_or(0, |s| s.reads),
            s.map_or(0.0, |s| s.total_us as f64 / s.reads.max(1) as f64 / 1000.0),
            s.map_or(0.0, |s| s.max_us as f64 / 1000.0),
            s.map_or(0, |s| s.errors),
            s.and_then(|s| s.last_error.as_ref())
                .map_or(String::new(), |e| format!("（最后一次：{e}）"))
        );
        if let Some(end) = counter_end[cam] {
            say!(
                "  Line0 跳变计数器结束时 = {end}，轮询看到的边沿共 {}（不相等说明有轮询漏掉的短脉冲，或计数器只数一种边沿）",
                edges.len()
            );
        }
        // 上升沿配下一个下降沿为一次门控；门控的帧 = 这次上升沿到下一次上升沿之间到达的帧
        let rises: Vec<usize> = (0..edges.len()).filter(|&i| edges[i].1).collect();
        let (mut first_lags, mut last_lags, mut after_max) = (Vec::new(), Vec::new(), 0usize);
        for (k, &r) in rises.iter().enumerate() {
            let Some(f) = (r + 1..edges.len()).find(|&i| !edges[i].1) else {
                continue;
            };
            let (rise, fall) = (&edges[r], &edges[f]);
            let next = rises.get(k + 1).map_or(f64::INFINITY, |&n| edges[n].0);
            let inside: Vec<&(f64, u64)> = frames
                .iter()
                .filter(|fr| fr.0 >= rise.0 - rise.3 && fr.0 < next)
                .collect();
            let (Some(first), Some(last)) = (inside.first(), inside.last()) else {
                say!(
                    "  门控 {:.3}s ~ {:.3}s：没有帧",
                    rise.0 / 1000.0,
                    fall.0 / 1000.0
                );
                continue;
            };
            let after = inside.iter().filter(|fr| fr.0 > fall.0).count();
            after_max = after_max.max(after);
            first_lags.push(first.0 - rise.0);
            last_lags.push(last.0 - fall.0);
            let clock = match (dev_ms(rise.2), dev_ms(fall.2), dev_ms(first.1), dev_ms(last.1)) {
                (Some(r), Some(f), Some(a), Some(b)) if rise.2 > 0 && fall.2 > 0 => format!(
                    "；相机时钟：门控 {:.1} ms ≈ {:.2} 个帧周期，首帧 − 上升沿 {:.1} ms，下降沿 − 末帧 {:.1} ms",
                    f - r,
                    (f - r) / period_ms,
                    a - r,
                    f - b
                ),
                _ => String::new(),
            };
            say!(
                "  门控 {:.3}s ~ {:.3}s：{} 帧；首帧到达 − 看到上升沿 {:.1} ms，末帧到达 − 看到下降沿 {:.1} ms，下降沿之后又来 {after} 帧{clock}",
                rise.0 / 1000.0,
                fall.0 / 1000.0,
                inside.len(),
                first.0 - rise.0,
                last.0 - fall.0
            );
        }
        if !last_lags.is_empty() {
            let span = |v: &mut Vec<f64>| {
                let (min, max) = (
                    v.iter().copied().fold(f64::INFINITY, f64::min),
                    v.iter().copied().fold(f64::NEG_INFINITY, f64::max),
                );
                format!(
                    "最小 {min:.1} / 中位 {:.1} / 最大 {max:.1} ms",
                    percentile(v, 0.5)
                )
            };
            say!(
                "  合计 {} 次门控：首帧到达 − 上升沿 {}；末帧到达 − 下降沿 {}（负数 = 末帧在看到下降沿之前就到了）；下降沿之后最多又来 {after_max} 帧",
                last_lags.len(),
                span(&mut first_lags),
                span(&mut last_lags)
            );
        }
    }
    say!("\n把日志与 CSV 发回即可。");
}
