use crate::config::Camera;
use serde::Serialize;
use std::sync::{
    atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    Arc,
};
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::broadcast;

#[derive(Clone)]
pub struct Frame {
    width: u32,
    height: u32,
    pixels: Arc<Vec<u8>>,
    pixel_format: &'static str,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct Header {
    width: u32,
    height: u32,
    frame_number: u64,
    timestamp_ns: u64,
    pixel_format: &'static str,
    payload_length: usize,
    gate_high: bool,
}

#[derive(Clone)]
struct Packet {
    header: Header,
    pixels: Arc<Vec<u8>>,
}

pub struct CameraServer {
    config: Camera,
    packets: broadcast::Sender<Packet>,
    pub connected: Arc<AtomicUsize>,
    pub gate: Arc<AtomicBool>,
    counter: Arc<AtomicU64>,
    epoch: Instant,
    task: tokio::task::JoinHandle<()>,
}

fn load(path: &std::path::Path) -> Result<Frame, String> {
    let image = image::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let color = image.color().has_color();
    let (width, height) = (image.width(), image.height());
    let pixels = if color {
        image.into_rgb8().into_raw()
    } else {
        image.into_luma8().into_raw()
    };
    Ok(Frame {
        width,
        height,
        pixels: Arc::new(pixels),
        pixel_format: if color { "rgb8" } else { "mono8" },
    })
}

impl CameraServer {
    pub async fn start(config: Camera, epoch: Instant) -> Result<Self, String> {
        let listener = TcpListener::bind(("127.0.0.1", config.port))
            .await
            .map_err(|e| format!("相机 {}: {e}", config.id))?;
        let (packets, _) = broadcast::channel::<Packet>(4);
        let connected = Arc::new(AtomicUsize::new(0));
        let gate = Arc::new(AtomicBool::new(false));
        let counter = Arc::new(AtomicU64::new(0));
        let preview = load(&config.frames[0])?;
        let (tx, clients, level, count) = (
            packets.clone(),
            connected.clone(),
            gate.clone(),
            counter.clone(),
        );
        let task = tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                if clients
                    .compare_exchange(0, 1, Ordering::SeqCst, Ordering::SeqCst)
                    .is_err()
                {
                    continue;
                }
                let (clients, level, count, preview, rx) = (
                    clients.clone(),
                    level.clone(),
                    count.clone(),
                    preview.clone(),
                    tx.subscribe(),
                );
                tokio::spawn(async move {
                    let _ = stream.set_nodelay(true);
                    let result = serve(stream, rx, &level, &count, epoch, preview).await;
                    clients.store(0, Ordering::SeqCst);
                    if let Err(e) = result {
                        eprintln!("相机连接结束：{e}");
                    }
                });
            }
        });
        Ok(Self {
            config,
            packets,
            connected,
            gate,
            counter,
            epoch,
            task,
        })
    }

    pub async fn prepare(&self, index: usize) -> Result<Frame, String> {
        let path = self.config.frames[index].clone();
        tokio::task::spawn_blocking(move || load(&path))
            .await
            .map_err(|e| e.to_string())?
    }

    pub fn send(&self, frame: Frame, timestamp_ns: u64) -> Result<(), String> {
        let frame_number = self.counter.fetch_add(1, Ordering::SeqCst) + 1;
        self.packets
            .send(Packet {
                header: Header {
                    width: frame.width,
                    height: frame.height,
                    frame_number,
                    timestamp_ns,
                    pixel_format: frame.pixel_format,
                    payload_length: frame.pixels.len(),
                    gate_high: self.gate.load(Ordering::SeqCst),
                },
                pixels: frame.pixels,
            })
            .map_err(|_| format!("相机 {} 未连接", self.config.id))?;
        Ok(())
    }

    pub fn set_gate(&self, high: bool) {
        self.gate.store(high, Ordering::SeqCst);
        let _ = self.packets.send(control(&self.counter, high, self.epoch));
    }
}

impl Drop for CameraServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn control(counter: &AtomicU64, high: bool, epoch: Instant) -> Packet {
    Packet {
        header: Header {
            width: 0,
            height: 0,
            frame_number: counter.load(Ordering::SeqCst),
            timestamp_ns: epoch.elapsed().as_nanos() as u64,
            pixel_format: "mono8",
            payload_length: 0,
            gate_high: high,
        },
        pixels: Arc::new(Vec::new()),
    }
}

async fn serve(
    stream: TcpStream,
    mut packets: broadcast::Receiver<Packet>,
    gate: &AtomicBool,
    counter: &AtomicU64,
    epoch: Instant,
    preview: Frame,
) -> Result<(), String> {
    let (read, mut write) = stream.into_split();
    let mut read = BufReader::new(read);
    let mut line = Vec::new();
    let mut tick = tokio::time::interval(Duration::from_millis(20));
    loop {
        let packet = tokio::select! {
            biased;
            packet = packets.recv() => packet.map_err(|e| e.to_string())?,
            _ = tick.tick() => control(counter, gate.load(Ordering::SeqCst), epoch),
            result = read_command(&mut read, &mut line) => {
                if result.map_err(|e| e.to_string())? == 0 { return Ok(()); }
                if line.len() > 1024 { return Err("相机命令过长".into()); }
                let command: serde_json::Value = serde_json::from_slice(&line).map_err(|e| e.to_string())?;
                line.clear();
                if command["command"] != "trigger" || gate.load(Ordering::SeqCst) { continue; }
                Packet { header: Header { width: preview.width, height: preview.height, frame_number: counter.fetch_add(1, Ordering::SeqCst) + 1, timestamp_ns: epoch.elapsed().as_nanos() as u64, pixel_format: preview.pixel_format, payload_length: preview.pixels.len(), gate_high: false }, pixels: preview.pixels.clone() }
            }
        };
        let header = serde_json::to_vec(&packet.header).map_err(|e| e.to_string())?;
        tokio::time::timeout(Duration::from_secs(2), async {
            write
                .write_all(&(header.len() as u32).to_be_bytes())
                .await?;
            write.write_all(&header).await?;
            write.write_all(&packet.pixels).await
        })
        .await
        .map_err(|_| "相机接收端过慢，连接已关闭")?
        .map_err(|e| e.to_string())?;
    }
}

async fn read_command(
    reader: &mut (impl AsyncBufRead + Unpin),
    line: &mut Vec<u8>,
) -> std::io::Result<usize> {
    reader
        .take(1025usize.saturating_sub(line.len()) as u64)
        .read_until(b'\n', line)
        .await
}
