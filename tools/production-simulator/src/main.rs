use gluesight_production_simulator::{
    camera::CameraServer,
    config::{init_demo, Config},
    plc::{production, Plc},
    s7_server::{BitAddr, S7Server},
};
use std::{
    path::Path,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};
use tokio::io::{AsyncBufReadExt, BufReader};

const USAGE: &str = "GlueSight 独立生产模拟器\n  --init <新目录>                    生成三相机灰度通路样本与配置\n  --config <simulator.json>          交互运行：run N / run 0（连续）/ stop（本件后停止）/ reset / quit\n  --config <simulator.json> --cycles N  自动运行N件（0为连续）\n  --config <simulator.json> --check   校验配置与图像，不监听端口\n工具仅监听127.0.0.1。模拟结果由GlueSight真实测量，不保证演示条图判OK。";

#[tokio::main]
async fn main() {
    if let Err(e) = run().await {
        eprintln!("{e}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), String> {
    let mut args = std::env::args().skip(1);
    let mut path = None;
    let mut cycles = None;
    let mut check = false;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--help" | "-h" => {
                println!("{USAGE}");
                return Ok(());
            }
            "--init" => {
                let dir = args.next().ok_or("--init 缺少目录")?;
                init_demo(Path::new(&dir))?;
                println!("已生成 {dir}/simulator.json");
                return Ok(());
            }
            "--config" => path = Some(args.next().ok_or("--config 缺少路径")?),
            "--cycles" => {
                cycles = Some(
                    args.next()
                        .ok_or("--cycles 缺少次数")?
                        .parse::<u32>()
                        .map_err(|_| "次数必须为非负整数")?,
                )
            }
            "--check" => check = true,
            _ => return Err(format!("未知参数 {arg}\n{USAGE}")),
        }
    }
    let config = Config::load(Path::new(&path.ok_or(USAGE)?))?;
    if check {
        println!(
            "配置及图像校验通过，{} 台相机，每件 {} 帧组",
            config.cameras.len(),
            config.pass_frames.iter().sum::<usize>()
        );
        return Ok(());
    }
    let epoch = Instant::now();
    let plc = Plc::new(config.clone(), epoch);
    let _s7 = S7Server::bind(
        config.plc_port,
        plc.memory.clone(),
        Some((
            BitAddr {
                db: config.plc_db,
                byte: 3,
                bit: 0,
            },
            Duration::from_millis(500),
        )),
    )
    .await
    .map_err(|e| e.to_string())?;
    let mut cameras = Vec::new();
    for camera in &config.cameras {
        cameras.push(CameraServer::start(camera.clone(), epoch).await?);
        println!(
            "相机 {} → 127.0.0.1:{}，source=simulator，Line0，{} fps",
            camera.id, camera.port, config.frame_rate
        );
    }
    println!(
        "PLC → 127.0.0.1:{}，DB{} / DB{}，型号 {}",
        config.plc_port, config.plc_db, config.pc_db, config.product_code
    );
    let cameras = Arc::new(cameras);
    let (monitor_plc, monitor_cameras) = (plc.clone(), cameras.clone());
    let monitor = tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_millis(10));
        loop {
            tick.tick().await;
            monitor_plc.poll(&monitor_cameras);
        }
    });
    let stop = Arc::new(AtomicBool::new(false));
    let sequence = Arc::new(AtomicU64::new(0));
    let result = if let Some(cycles) = cycles {
        tokio::select! {
            result = production(plc.clone(), cameras.clone(), cycles, stop.clone(), sequence.clone()) => result,
            _ = tokio::signal::ctrl_c() => Ok(()),
        }
    } else {
        println!("命令：run N / run 0（连续）/ stop（本件后停止）/ reset / quit");
        let mut lines = BufReader::new(tokio::io::stdin()).lines();
        let mut job: Option<tokio::task::JoinHandle<Result<(), String>>> = None;
        loop {
            if job.as_ref().is_some_and(|task| task.is_finished()) {
                match job.take().unwrap().await {
                    Ok(Ok(())) => println!("运行已结束"),
                    Ok(Err(e)) => eprintln!("{e}"),
                    Err(e) => eprintln!("{e}"),
                }
            }
            let line = tokio::select! {
                line = lines.next_line() => line.map_err(|e| e.to_string())?,
                _ = tokio::signal::ctrl_c() => None,
            };
            let Some(line) = line else {
                break;
            };
            let words: Vec<_> = line.split_whitespace().collect();
            match words.as_slice() {
                ["run", count] => {
                    if job.as_ref().is_some_and(|task| !task.is_finished()) {
                        eprintln!("已有运行任务");
                        continue;
                    }
                    let Ok(count) = count.parse::<u32>() else {
                        eprintln!("次数必须为非负整数");
                        continue;
                    };
                    stop.store(false, Ordering::SeqCst);
                    job = Some(tokio::spawn(production(
                        plc.clone(),
                        cameras.clone(),
                        count,
                        stop.clone(),
                        sequence.clone(),
                    )));
                }
                ["stop"] => {
                    stop.store(true, Ordering::SeqCst);
                }
                ["reset"] => {
                    for camera in cameras.iter() {
                        camera.set_gate(false);
                    }
                    plc.request_reset();
                }
                ["quit"] => break,
                [] => {}
                _ => eprintln!("命令：run N / stop / reset / quit"),
            }
        }
        if let Some(job) = job {
            job.abort();
        }
        Ok(())
    };
    for camera in cameras.iter() {
        camera.set_gate(false);
    }
    monitor.abort();
    result
}
