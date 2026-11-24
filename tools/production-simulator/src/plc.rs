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

const SEQUENCE_MAX: u64 = i32::MAX as u64;

pub struct Plc {
    pub memory: SharedMemory,
    pub config: Config,
    epoch: Instant,
    heartbeat: Mutex<(u16, Option<Instant>)>,
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Outcome {
    pub sequence: u32,
    pub sn: String,
    pub result: u16,
    pub fault: u16,
    pub maintenance: bool,
    pub sn_echo_matches: bool,
}

impl Plc {
    pub fn new(config: Config, epoch: Instant) -> Arc<Self> {
        let mut memory = Memory::default();
        memory.create(config.plc_db, 68);
        memory.create(config.pc_db, 68);
        memory.write(config.plc_db, 0, &9u16.to_be_bytes()).unwrap();
        memory.write(config.plc_db, 16, &[50, 0]).unwrap();
        memory.write(config.pc_db, 16, &[50, 0]).unwrap();
        Arc::new(Self {
            memory: Arc::new(Mutex::new(memory)),
            config,
            epoch,
            heartbeat: Mutex::new((0, None)),
        })
    }

    fn pc_alive(&self) -> bool {
        let bytes = self
            .memory
            .lock()
            .unwrap()
            .read(self.config.pc_db, 2, 2)
            .unwrap();
        let value = u16::from_be_bytes([bytes[0], bytes[1]]);
        let mut seen = self.heartbeat.lock().unwrap();
        if value != seen.0 {
            *seen = (value, Some(Instant::now()));
        }
        seen.1
            .is_some_and(|changed| changed.elapsed() < Duration::from_secs(3))
    }

    async fn hold(&self, until: Instant) {
        loop {
            self.pc_alive();
            let left = until.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return;
            }
            tokio::time::sleep(left.min(Duration::from_millis(10))).await;
        }
    }

    fn inspecting(&self, value: bool) {
        self.memory
            .lock()
            .unwrap()
            .set_bit(self.config.plc_db, 4, 0, value)
            .unwrap();
    }

    pub async fn run(
        &self,
        sequence: u32,
        sn: String,
        cameras: &[CameraServer],
    ) -> Result<Option<Outcome>, String> {
        {
            let mut text = [0u8; 52];
            text[0] = 50;
            text[1] = sn.len() as u8;
            text[2..2 + sn.len()].copy_from_slice(sn.as_bytes());
            let db = self.config.plc_db;
            let mut memory = self.memory.lock().unwrap();
            memory
                .write(db, 12, &self.config.product_code.to_be_bytes())
                .unwrap();
            memory.write(db, 16, &text).unwrap();
            memory.write(db, 8, &sequence.to_be_bytes()).unwrap();
            memory.set_bit(db, 4, 0, true).unwrap();
        }
        let captured = self.capture(cameras).await;
        for camera in cameras {
            camera.set_gate(false);
        }
        self.inspecting(false);
        captured?;
        let deadline = Instant::now() + Duration::from_millis(self.config.result_timeout_ms);
        loop {
            let alive = self.pc_alive();
            let bytes = self
                .memory
                .lock()
                .unwrap()
                .read(self.config.pc_db, 0, 68)
                .unwrap();
            if alive && u32::from_be_bytes(bytes[8..12].try_into().unwrap()) == sequence {
                let sn_echo_matches = bytes.get(18..18 + bytes[17] as usize) == Some(sn.as_bytes());
                return Ok(Some(Outcome {
                    sequence,
                    sn,
                    result: u16::from_be_bytes([bytes[12], bytes[13]]),
                    fault: u16::from_be_bytes([bytes[14], bytes[15]]),
                    maintenance: u16::from_be_bytes([bytes[6], bytes[7]]) == 1,
                    sn_echo_matches,
                }));
            }
            if Instant::now() >= deadline {
                return Ok(None);
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    async fn capture(&self, cameras: &[CameraServer]) -> Result<(), String> {
        self.hold(Instant::now() + Duration::from_millis(self.config.capture_lead_ms))
            .await;
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
                self.hold(start + period * frame as u32).await;
                let stamp = self.epoch.elapsed().as_nanos() as u64;
                for (camera, image) in cameras.iter().zip(prepared) {
                    camera.send(image, stamp)?;
                }
                index += 1;
            }
            self.hold(start + period * *count as u32).await;
            for camera in cameras {
                camera.set_gate(false);
            }
            if pass + 1 < self.config.pass_frames.len() {
                self.hold(Instant::now() + Duration::from_millis(self.config.pass_gap_ms))
                    .await;
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
    while cameras
        .iter()
        .any(|c| c.connected.load(Ordering::SeqCst) == 0)
    {
        if stop.load(Ordering::SeqCst) {
            return Ok(());
        }
        if start.elapsed() > Duration::from_secs(60) {
            return Err("60秒内相机未全部连接，请检查应用的相机来源与地址".into());
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let mut attempted = 0;
    let mut missed = 0;
    while cycles == 0 || attempted < cycles {
        if stop.load(Ordering::SeqCst) {
            break;
        }
        let seq = sequence.update(Ordering::SeqCst, Ordering::SeqCst, |n| n % SEQUENCE_MAX + 1)
            % SEQUENCE_MAX
            + 1;
        let sn = format!("{}-{seq:010}", plc.config.sn_prefix);
        match plc.run(seq as u32, sn.clone(), &cameras).await {
            Ok(Some(outcome)) => println!("{}", serde_json::to_string(&outcome).unwrap()),
            Ok(None) => {
                missed += 1;
                println!(
                    "{}",
                    serde_json::json!({"sn": sn, "status": "未检：未取到结果"})
                );
            }
            Err(error) => {
                missed += 1;
                eprintln!("{sn}: {error}");
            }
        }
        attempted += 1;
        tokio::time::sleep(Duration::from_millis(plc.config.interval_ms)).await;
    }
    if missed > 0 {
        Err(format!("本批 {attempted} 件中 {missed} 件未取到结果"))
    } else {
        Ok(())
    }
}
