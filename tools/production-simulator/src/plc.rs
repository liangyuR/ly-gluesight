use crate::{
    camera::CameraServer,
    config::Config,
    s7_server::{Memory, SharedMemory},
};
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Arc, Mutex,
};
use std::time::{Duration, Instant};

pub struct Plc {
    pub memory: SharedMemory,
    pub config: Config,
    epoch: Instant,
    heartbeat: Mutex<(u16, Instant, bool)>,
    reset: AtomicU64,
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Outcome {
    pub sequence: u32,
    pub sn: String,
    pub result: u16,
    pub fault: u16,
    pub maintenance: bool,
}

impl Plc {
    pub fn new(config: Config, epoch: Instant) -> Arc<Self> {
        let mut memory = Memory::default();
        memory.create(config.plc_db, 68);
        memory.create(config.pc_db, 68);
        memory.write(config.plc_db, 0, &8u16.to_be_bytes()).unwrap();
        memory.write(config.plc_db, 16, &[50, 0]).unwrap();
        memory.write(config.pc_db, 16, &[50, 0]).unwrap();
        Arc::new(Self {
            memory: Arc::new(Mutex::new(memory)),
            config,
            epoch,
            heartbeat: Mutex::new((0, epoch, false)),
            reset: AtomicU64::new(0),
        })
    }

    pub fn pc(&self, byte: u32) -> u16 {
        self.word(self.config.pc_db, byte)
    }

    fn word(&self, db: u16, byte: u32) -> u16 {
        let bytes = self.memory.lock().unwrap().read(db, byte, 2).unwrap();
        u16::from_be_bytes([bytes[0], bytes[1]])
    }

    pub fn state(&self, state: u16, reason: u16) {
        let mut memory = self.memory.lock().unwrap();
        memory
            .write(self.config.plc_db, 6, &reason.to_be_bytes())
            .unwrap();
        memory
            .write(self.config.plc_db, 4, &state.to_be_bytes())
            .unwrap();
    }

    pub fn poll(&self, cameras: &[CameraServer]) {
        let heartbeat = self.pc(2);
        let mut seen = self.heartbeat.lock().unwrap();
        if heartbeat != seen.0 {
            *seen = (heartbeat, Instant::now(), true);
        }
        drop(seen);
        if self.pc(4) == 99 {
            if self.word(self.config.plc_db, 4) != 0 {
                for camera in cameras {
                    camera.set_gate(false);
                }
                self.state(0, 0);
                self.reset.fetch_add(1, Ordering::SeqCst);
            }
        } else if self.word(self.config.plc_db, 4) == 99 && self.pc(4) == 10 {
            self.state(0, 0);
            self.reset.fetch_add(1, Ordering::SeqCst);
        }
    }

    pub fn ready(&self) -> bool {
        let seen = self.heartbeat.lock().unwrap();
        seen.2
            && seen.1.elapsed() < Duration::from_secs(3)
            && self.pc(0) == 8
            && self.pc(4) == 10
            && self.word(self.config.plc_db, 4) == 0
    }

    fn running(&self, generation: u64) -> Result<(), String> {
        if self.reset.load(Ordering::SeqCst) != generation || self.pc(4) == 99 {
            return Err("PC 复位中，本件已中断".into());
        }
        if matches!(self.pc(4), 0 | 90) || self.pc(0) != 8 {
            self.state(90, 14);
            return Err("PC 未就绪、故障或协议版本不匹配".into());
        }
        if self.heartbeat.lock().unwrap().1.elapsed() >= Duration::from_secs(3) {
            self.state(90, 6);
            return Err("PC 心跳失活".into());
        }
        Ok(())
    }

    async fn hold(&self, duration: Duration, generation: u64) -> Result<(), String> {
        let until = Instant::now() + duration;
        loop {
            self.running(generation)?;
            if Instant::now() >= until {
                return Ok(());
            }
            tokio::time::sleep(
                Duration::from_millis(10).min(until.saturating_duration_since(Instant::now())),
            )
            .await;
        }
    }

    pub fn request_reset(&self) {
        self.state(99, 0);
    }

    pub async fn run(
        &self,
        sequence: u32,
        sn: String,
        cameras: &[CameraServer],
    ) -> Result<Option<Outcome>, String> {
        if !self.ready() {
            return Ok(None);
        }
        let generation = self.reset.load(Ordering::SeqCst);
        {
            let mut memory = self.memory.lock().unwrap();
            let mut text = [0u8; 52];
            text[0] = 50;
            text[1] = sn.len() as u8;
            text[2..2 + sn.len()].copy_from_slice(sn.as_bytes());
            for (byte, bytes) in [
                (8, sequence.to_be_bytes().to_vec()),
                (12, self.config.product_code.to_be_bytes().to_vec()),
                (16, text.to_vec()),
                (6, 0u16.to_be_bytes().to_vec()),
                (4, 10u16.to_be_bytes().to_vec()),
            ] {
                memory.write(self.config.plc_db, byte, &bytes).unwrap();
            }
        }
        let result = self.capture(generation, cameras).await;
        for camera in cameras {
            camera.set_gate(false);
        }
        if let Err(error) = result {
            if self.reset.load(Ordering::SeqCst) == generation && self.pc(4) != 99 {
                self.state(90, 14);
            }
            return Err(error);
        }
        self.state(20, 0);
        let deadline = Instant::now() + Duration::from_millis(self.config.result_timeout_ms);
        let mut previous = None;
        loop {
            self.running(generation)?;
            if self.pc(4) == 30 {
                let bytes = self
                    .memory
                    .lock()
                    .unwrap()
                    .read(self.config.pc_db, 0, 68)
                    .unwrap();
                if previous.as_ref() == Some(&bytes) {
                    let echoed_sequence = u32::from_be_bytes(bytes[8..12].try_into().unwrap());
                    let result = u16::from_be_bytes(bytes[12..14].try_into().unwrap());
                    let fault = u16::from_be_bytes(bytes[14..16].try_into().unwrap());
                    let mode = u16::from_be_bytes(bytes[6..8].try_into().unwrap());
                    let text_len = bytes[17] as usize;
                    let echoed_sn = if text_len <= 50 {
                        &bytes[18..18 + text_len]
                    } else {
                        &[]
                    };
                    let valid = if mode == 1 {
                        result == 0 && fault == 0
                    } else {
                        mode == 0
                            && (matches!(result, 1 | 2 | 11..=14) && fault == 0
                                || result == 90 && (91..=100).contains(&fault))
                    };
                    if echoed_sequence != sequence || echoed_sn != sn.as_bytes() || !valid {
                        self.state(90, 5);
                        return Err("PC 结果的序号、SN 或结果组合无效，未确认".into());
                    }
                    self.state(30, 0);
                    let release = Instant::now() + Duration::from_secs(10);
                    while self.pc(4) != 10 {
                        self.running(generation)?;
                        if Instant::now() >= release {
                            self.state(90, 10);
                            return Err("PC 未在结果确认后回就绪".into());
                        }
                        tokio::time::sleep(Duration::from_millis(20)).await;
                    }
                    self.state(0, 0);
                    return Ok(Some(Outcome {
                        sequence,
                        sn,
                        result,
                        fault,
                        maintenance: mode == 1,
                    }));
                }
                previous = Some(bytes);
            } else {
                previous = None;
            }
            if Instant::now() >= deadline {
                self.state(90, 8);
                return Err("工件结束后等待 PC 结果超时".into());
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    async fn capture(&self, generation: u64, cameras: &[CameraServer]) -> Result<(), String> {
        self.hold(
            Duration::from_millis(self.config.capture_lead_ms),
            generation,
        )
        .await?;
        let mut index = 0;
        let period = Duration::from_secs_f64(1.0 / self.config.frame_rate);
        for (pass, count) in self.config.pass_frames.iter().enumerate() {
            let start = Instant::now();
            for camera in cameras {
                camera.set_gate(true);
            }
            for frame in 0..*count {
                let mut prepared = Vec::new();
                for camera in cameras {
                    prepared.push(camera.prepare(index).await?);
                }
                let deadline = start + period * frame as u32;
                self.hold(
                    deadline.saturating_duration_since(Instant::now()),
                    generation,
                )
                .await?;
                let stamp = self.epoch.elapsed().as_nanos() as u64;
                for (camera, image) in cameras.iter().zip(prepared) {
                    camera.send(image, stamp)?;
                }
                index += 1;
            }
            self.hold(
                (start + period * *count as u32).saturating_duration_since(Instant::now()),
                generation,
            )
            .await?;
            for camera in cameras {
                camera.set_gate(false);
            }
            if pass + 1 < self.config.pass_frames.len() {
                self.hold(Duration::from_millis(self.config.pass_gap_ms), generation)
                    .await?;
            }
        }
        Ok(())
    }
}

pub async fn production(
    plc: Arc<Plc>,
    cameras: Arc<Vec<CameraServer>>,
    cycles: u32,
    stop: Arc<AtomicBool>,
    sequence: Arc<AtomicU64>,
) -> Result<(), String> {
    let start = Instant::now();
    while !plc.ready()
        || cameras
            .iter()
            .any(|c| c.connected.load(Ordering::SeqCst) == 0)
    {
        if stop.load(Ordering::SeqCst) {
            return Ok(());
        }
        if start.elapsed() > Duration::from_secs(60) {
            return Err("60秒内 PLC/相机未就绪，请检查应用的PLC连接、相机地址与软件复位".into());
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let mut attempted = 0;
    let mut failed = 0;
    while cycles == 0 || attempted < cycles {
        if stop.load(Ordering::SeqCst) {
            break;
        }
        let seq = sequence.fetch_add(1, Ordering::SeqCst) + 1;
        if seq > i32::MAX as u64 {
            plc.state(90, 12);
            return Err("事务序号耗尽，请协调复位并重启工具".into());
        }
        let sn = format!("{}-{seq:010}", plc.config.sn_prefix);
        match plc.run(seq as u32, sn.clone(), &cameras).await {
            Ok(Some(outcome)) => println!("{}", serde_json::to_string(&outcome).unwrap()),
            Ok(None) => {
                failed += 1;
                println!(
                    "{}",
                    serde_json::json!({"sn": sn, "status": "未检：PC未就绪"})
                );
            }
            Err(error) => {
                failed += 1;
                for camera in cameras.iter() {
                    camera.set_gate(false);
                }
                eprintln!("{sn}: {error}");
            }
        }
        attempted += 1;
        tokio::time::sleep(Duration::from_millis(plc.config.interval_ms)).await;
    }
    if failed > 0 {
        Err(format!(
            "本批 {attempted} 件中 {failed} 件未完成生产握手，见逐件错误"
        ))
    } else {
        Ok(())
    }
}
