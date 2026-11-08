use std::io::Read;
use std::net::{SocketAddr, TcpStream};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::Deserialize;

use super::{CameraConfig, CameraSlot, Frame, Shared};
use crate::frame::{CounterSource, FrameImage};

const MAX_HEADER: usize = 4096;
const MAX_IMAGE: usize = 64 * 1024 * 1024;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Header {
    width: u32,
    height: u32,
    frame_number: u64,
    timestamp_ns: u64,
    pixel_format: String,
    payload_length: usize,
    gate_high: bool,
}

pub(super) fn endpoint(value: &str) -> Result<SocketAddr, String> {
    let address: SocketAddr = value
        .parse()
        .map_err(|_| "模拟器地址需为 IP:端口，例如 127.0.0.1:7101")?;
    if !address.ip().is_loopback() || address.port() == 0 {
        return Err("模拟器只接受本机回环地址与非零端口".into());
    }
    Ok(address)
}

impl Header {
    fn channels(&self) -> Result<usize, String> {
        let channels = match self.pixel_format.as_str() {
            "mono8" => 1,
            "rgb8" => 3,
            _ => return Err("模拟器像素格式必须是 mono8 或 rgb8".into()),
        };
        if self.width == 0 && self.height == 0 && self.payload_length == 0 {
            return Ok(channels);
        }
        let size = (self.width as usize)
            .checked_mul(self.height as usize)
            .and_then(|n| n.checked_mul(channels));
        if self.width == 0
            || self.height == 0
            || self.width > 16384
            || self.height > 16384
            || self.payload_length > MAX_IMAGE
            || size != Some(self.payload_length)
        {
            return Err("模拟器图像尺寸或像素长度无效".into());
        }
        Ok(channels)
    }
}

fn read_packet(stream: &mut impl Read, bytes: &mut Vec<u8>) -> Result<(Header, usize), String> {
    let mut length = [0; 4];
    stream.read_exact(&mut length).map_err(|e| e.to_string())?;
    let length = u32::from_be_bytes(length) as usize;
    if length == 0 || length > MAX_HEADER {
        return Err("模拟器帧头长度无效".into());
    }
    bytes.resize(length, 0);
    stream.read_exact(bytes).map_err(|e| e.to_string())?;
    let header: Header =
        serde_json::from_slice(bytes).map_err(|e| format!("模拟器帧头无效：{e}"))?;
    let channels = header.channels()?;
    bytes.resize(header.payload_length, 0);
    stream.read_exact(bytes).map_err(|e| e.to_string())?;
    Ok((header, channels))
}

pub(super) fn open(slot: &CameraSlot, config: &CameraConfig) -> Result<String, String> {
    let mut stream = TcpStream::connect_timeout(&endpoint(&config.serial)?, Duration::from_secs(2))
        .map_err(|e| format!("连接模拟器失败：{e}"))?;
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .map_err(|e| e.to_string())?;
    stream
        .set_write_timeout(Some(Duration::from_secs(2)))
        .map_err(|e| e.to_string())?;
    stream.set_nodelay(true).map_err(|e| e.to_string())?;
    let mut bytes = Vec::new();
    let (first, channels) = read_packet(&mut stream, &mut bytes)?;
    slot.new_session();
    let session = slot.shared.session.load(Ordering::SeqCst);
    slot.shared.disconnected.store(false, Ordering::SeqCst);
    *slot.simulator.lock().unwrap() = Some(stream.try_clone().map_err(|e| e.to_string())?);
    let shared = slot.shared.clone();
    // 控制包的当前计数用于接件基线，不会被当成生产图像。
    shared
        .identity
        .lock()
        .unwrap()
        .observe(&crate::shot_router::FrameMeta::from(&frame(
            &shared, session, &first,
        )));
    let message = format!("已连接外部模拟器 {}", config.serial);
    {
        let mut state = slot.state.lock().unwrap();
        state.message = message.clone();
        state.configured_frame_rate = Some(config.frame_rate);
    }
    std::thread::spawn(move || {
        let result = receive(&mut stream, &shared, session, first, channels, &mut bytes);
        if shared.session.load(Ordering::SeqCst) == session {
            shared.disconnected.store(true, Ordering::SeqCst);
            shared.rig.gate.forget(shared.cam);
            if let Err(error) = result {
                super::cycle::log(&shared.rig.app, "err", "模拟器相机", error);
            }
        }
    });
    Ok(message)
}

fn frame(shared: &Shared, session: u64, header: &Header) -> Frame {
    Frame {
        cam: shared.cam,
        session,
        counter: CounterSource::SdkFrame,
        frame_counter: header.frame_number,
        trigger_counter: header.frame_number,
        lost_packets: 0,
        ts: ly_plc::now_ms(),
        dev_ts: Some(header.timestamp_ns),
        manual: false,
        images: Vec::new(),
        bayer: None,
    }
}

fn receive(
    stream: &mut TcpStream,
    shared: &Arc<Shared>,
    session: u64,
    mut header: Header,
    mut channels: usize,
    bytes: &mut Vec<u8>,
) -> Result<(), String> {
    let mut gate = None;
    let mut previous_frame = None;
    let mut previous_clock = None;
    loop {
        if shared.session.load(Ordering::SeqCst) != session {
            return Ok(());
        }
        let now = Instant::now();
        {
            let mut clock = shared.simulator_clock.lock().unwrap();
            if clock.is_none_or(|(previous, at)| {
                header.timestamp_ns
                    >= previous.saturating_add(now.duration_since(at).as_nanos() as u64)
            }) {
                *clock = Some((header.timestamp_ns, now));
            }
        }
        if gate != Some(header.gate_high) {
            shared
                .rig
                .gate
                .edge(shared.cam, header.gate_high, header.timestamp_ns, now);
            gate = Some(header.gate_high);
        }
        shared.rig.gate.observe(shared.cam, header.gate_high, now);
        if header.payload_length > 0 {
            if previous_clock.is_some_and(|clock| header.timestamp_ns < clock) {
                return Err("模拟器图像时间戳回退，请重连".into());
            }
            previous_clock = Some(header.timestamp_ns);
            if previous_frame.is_some_and(|number| header.frame_number <= number) {
                return Err("模拟器图像帧号未递增，请重连".into());
            }
            previous_frame = Some(header.frame_number);
            let mut delivered = frame(shared, session, &header);
            let pixels = if channels == 3 && !shared.color.load(Ordering::Relaxed) {
                let mut gray = Vec::with_capacity(bytes.len() / 3);
                gray.extend(bytes.chunks_exact(3).map(|p| {
                    ((77 * p[0] as u32 + 150 * p[1] as u32 + 29 * p[2] as u32 + 128) >> 8) as u8
                }));
                gray
            } else {
                Vec::new()
            };
            let (source, count) = if !pixels.is_empty() {
                (pixels.as_slice(), 1)
            } else {
                (bytes.as_slice(), channels)
            };
            if shared.color.load(Ordering::Relaxed) && count != 3 {
                return Err("彩色模式需要模拟器发送 rgb8 图像".into());
            }
            match shared.pool.copy(header.width, header.height, source) {
                Ok(mut image) => {
                    image.channels = count as u8;
                    let image: Arc<FrameImage> = Arc::new(image);
                    delivered.images.push(image);
                    shared.set_previews(session, &delivered.images);
                    shared.deliver(delivered);
                }
                Err(_) => shared.discard_over_budget(delivered),
            }
        }
        (header, channels) = read_packet(stream, bytes)?;
    }
}
