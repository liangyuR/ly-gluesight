use std::collections::BTreeMap;
use std::io;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;
use tokio::task::JoinHandle;

/// S7-1200 协商 240 字节 PDU，离线服务按现场 CPU 限制应答。
const MAX_PDU: u16 = 240;
const REJECTED: [u8; 2] = [0x81, 0x04];
const TOO_LARGE: [u8; 2] = [0x85, 0x00];
const AREA_DB: u8 = 0x84;

#[derive(Default)]
pub struct Memory {
    blocks: BTreeMap<u16, Vec<u8>>,
}

pub type SharedMemory = Arc<Mutex<Memory>>;

impl Memory {
    pub fn create(&mut self, db: u16, size: usize) {
        self.blocks.insert(db, vec![0; size]);
    }

    fn range(&self, db: u16, byte: u32, len: usize) -> Result<std::ops::Range<usize>, u8> {
        let block = self.blocks.get(&db).ok_or(0x0A)?;
        let start = byte as usize;
        let end = start.checked_add(len).filter(|end| *end <= block.len()).ok_or(0x05)?;
        Ok(start..end)
    }

    pub fn read(&self, db: u16, byte: u32, len: usize) -> Result<Vec<u8>, u8> {
        let range = self.range(db, byte, len)?;
        Ok(self.blocks[&db][range].to_vec())
    }

    pub fn write(&mut self, db: u16, byte: u32, data: &[u8]) -> Result<(), u8> {
        let range = self.range(db, byte, data.len())?;
        self.blocks.get_mut(&db).unwrap()[range].copy_from_slice(data);
        Ok(())
    }

    pub fn bit(&self, db: u16, byte: u32, bit: u8) -> Result<bool, u8> {
        Ok((self.read(db, byte, 1)?[0] >> bit) & 1 == 1)
    }

    pub fn set_bit(&mut self, db: u16, byte: u32, bit: u8, value: bool) -> Result<(), u8> {
        let current = self.read(db, byte, 1)?[0];
        let mask = 1 << bit;
        self.write(db, byte, &[if value { current | mask } else { current & !mask }])
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BitAddr {
    pub db: u16,
    pub byte: u32,
    pub bit: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fault {
    /// 立即断开当前所有连接，客户端按自己的重连周期重新连接
    Disconnect,
    RejectRead,
    RejectWrite,
    Delay(Duration),
    HeartbeatStop,
    HeartbeatResume,
    /// PLC 停机：断开当前连接，并在这段时间内拒绝新连接
    Outage(Duration),
    Clear,
    /// 测试用：拒绝下一次写到该地址的那一项（同一请求里的其它项照常写入）
    #[cfg(test)]
    RejectWriteTo(WriteTarget),
    /// 测试用：下一次写到该地址的请求照常写入，但不应答并断开连接，PC 无法确认写入结果
    #[cfg(test)]
    DisconnectAfterWriteTo(WriteTarget),
}

/// 定点写故障的目标；不带位号时匹配覆盖该字节的字节项，带位号时只匹配该位的位写入。
#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WriteTarget {
    pub db: u16,
    pub byte: u32,
    pub bit: Option<u8>,
}

#[cfg(test)]
impl WriteTarget {
    fn covers(&self, item: &Item) -> bool {
        item.db == self.db && match self.bit {
            Some(bit) => item.transport == 1 && item.byte() == self.byte && item.bit() == bit,
            None => (item.byte()..item.byte() + item.size() as u32).contains(&self.byte),
        }
    }
}

#[derive(Default)]
struct Control {
    reject_reads: AtomicU32,
    reject_writes: AtomicU32,
    delays: AtomicU32,
    delay_ms: AtomicU64,
    heartbeat_stopped: AtomicBool,
    outage_until: Mutex<Option<Instant>>,
    /// 定点写故障：(目标, 写完后是否断开)，各生效一次
    #[cfg(test)]
    targets: Mutex<Vec<(WriteTarget, bool)>>,
}

impl Control {
    fn take(counter: &AtomicU32) -> bool {
        counter.try_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1)).is_ok()
    }

    fn in_outage(&self) -> bool {
        self.outage_until.lock().unwrap().is_some_and(|until| Instant::now() < until)
    }

    /// 取出这次写请求命中的第一个定点写故障：(某项是否被拒, 写完后是否断开)
    #[cfg(test)]
    fn take_target(&self, items: &[Item]) -> (impl Fn(&Item) -> bool, bool) {
        let mut targets = self.targets.lock().unwrap();
        let taken = targets.iter().position(|(target, _)| items.iter().any(|item| target.covers(item))).map(|index| targets.remove(index));
        let rejected = taken.filter(|(_, disconnect)| !disconnect).map(|(target, _)| target);
        (move |item: &Item| rejected.is_some_and(|target| target.covers(item)), taken.is_some_and(|(_, disconnect)| disconnect))
    }

    #[cfg(not(test))]
    fn take_target(&self, _items: &[Item]) -> (impl Fn(&Item) -> bool, bool) {
        (|_: &Item| false, false)
    }
}

/// 进程内 S7 服务：只绑定 127.0.0.1，端口由系统分配；实现 ly-plc S7 客户端用到的 COTP 连接、通讯协商与 DB 区 ReadVar / WriteVar。
pub struct S7Server {
    port: u16,
    control: Arc<Control>,
    epoch: watch::Sender<u64>,
    tasks: Vec<JoinHandle<()>>,
}

impl S7Server {
    pub async fn start(memory: SharedMemory, heartbeat: Option<(BitAddr, Duration)>) -> io::Result<Self> {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
        let port = listener.local_addr()?.port();
        let control = Arc::new(Control::default());
        let (epoch, _) = watch::channel(0u64);
        let mut tasks = Vec::new();
        let (accept_memory, accept_control, accept_epoch) = (memory.clone(), control.clone(), epoch.clone());
        tasks.push(tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                if accept_control.in_outage() { continue; }
                let _ = stream.set_nodelay(true);
                tokio::spawn(serve(stream, accept_memory.clone(), accept_control.clone(), accept_epoch.subscribe()));
            }
        }));
        if let Some((address, interval)) = heartbeat {
            let (memory, control) = (memory.clone(), control.clone());
            tasks.push(tokio::spawn(async move {
                let mut ticker = tokio::time::interval(interval);
                ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
                loop {
                    ticker.tick().await;
                    if control.heartbeat_stopped.load(Ordering::SeqCst) { continue; }
                    let mut memory = memory.lock().unwrap();
                    if let Ok(level) = memory.bit(address.db, address.byte, address.bit) {
                        let _ = memory.set_bit(address.db, address.byte, address.bit, !level);
                    }
                }
            }));
        }
        Ok(Self { port, control, epoch, tasks })
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    pub fn heartbeat_stopped(&self) -> bool {
        self.control.heartbeat_stopped.load(Ordering::SeqCst)
    }

    /// 还没有被写请求命中的定点写故障数
    #[cfg(test)]
    pub fn armed_write_faults(&self) -> usize {
        self.control.targets.lock().unwrap().len()
    }

    pub fn inject(&self, fault: Fault) {
        let control = &self.control;
        match fault {
            Fault::Disconnect => { self.epoch.send_modify(|epoch| *epoch += 1); }
            Fault::RejectRead => { control.reject_reads.fetch_add(1, Ordering::SeqCst); }
            Fault::RejectWrite => { control.reject_writes.fetch_add(1, Ordering::SeqCst); }
            Fault::Delay(delay) => {
                control.delay_ms.store(delay.as_millis() as u64, Ordering::SeqCst);
                control.delays.fetch_add(1, Ordering::SeqCst);
            }
            Fault::HeartbeatStop => control.heartbeat_stopped.store(true, Ordering::SeqCst),
            Fault::HeartbeatResume => control.heartbeat_stopped.store(false, Ordering::SeqCst),
            Fault::Outage(duration) => {
                *control.outage_until.lock().unwrap() = Some(Instant::now() + duration);
                self.epoch.send_modify(|epoch| *epoch += 1);
            }
            Fault::Clear => {
                for counter in [&control.reject_reads, &control.reject_writes, &control.delays] { counter.store(0, Ordering::SeqCst); }
                control.outage_until.lock().unwrap().take();
                #[cfg(test)]
                control.targets.lock().unwrap().clear();
            }
            #[cfg(test)]
            Fault::RejectWriteTo(target) => control.targets.lock().unwrap().push((target, false)),
            #[cfg(test)]
            Fault::DisconnectAfterWriteTo(target) => control.targets.lock().unwrap().push((target, true)),
        }
    }
}

impl Drop for S7Server {
    fn drop(&mut self) {
        for task in &self.tasks { task.abort(); }
        self.epoch.send_modify(|epoch| *epoch += 1);
    }
}

async fn read_packet(stream: &mut TcpStream) -> io::Result<Vec<u8>> {
    let mut head = [0u8; 4];
    stream.read_exact(&mut head).await?;
    let len = u16::from_be_bytes([head[2], head[3]]) as usize;
    if head[..2] != [0x03, 0x00] || len < 7 {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "TPKT 帧头异常"));
    }
    let mut packet = vec![0u8; len];
    packet[..4].copy_from_slice(&head);
    stream.read_exact(&mut packet[4..]).await?;
    Ok(packet)
}

async fn serve(mut stream: TcpStream, memory: SharedMemory, control: Arc<Control>, mut epoch: watch::Receiver<u64>) {
    let mut connected = false;
    let mut pdu = None;
    loop {
        let packet = tokio::select! {
            packet = read_packet(&mut stream) => match packet { Ok(packet) => packet, Err(_) => return },
            _ = epoch.changed() => return,
        };
        let body = &packet[4..];
        let response = if !connected {
            connected = true;
            match connect_response(body) { Some(response) => response, None => return }
        } else {
            if body.get(..3) != Some(&[0x02, 0xF0, 0x80]) { return; }
            if Control::take(&control.delays) {
                tokio::select! {
                    _ = tokio::time::sleep(Duration::from_millis(control.delay_ms.load(Ordering::SeqCst))) => {}
                    _ = epoch.changed() => return,
                }
            }
            match exchange(&body[3..], &mut pdu, &memory, &control) { Some(response) => response, None => return }
        };
        if stream.write_all(&response).await.is_err() { return; }
    }
}

fn tpkt(payload: &[u8]) -> Vec<u8> {
    let mut frame = vec![0x03, 0x00];
    frame.extend_from_slice(&((payload.len() + 4) as u16).to_be_bytes());
    frame.extend_from_slice(payload);
    frame
}

fn connect_response(body: &[u8]) -> Option<Vec<u8>> {
    if body.len() < 7 || body[0] as usize != body.len() - 1 || body[1] != 0xE0 || body[6] != 0 { return None; }
    let mut cursor = 7;
    let (mut local, mut remote) = (None, None);
    while cursor < body.len() {
        let (code, size) = (*body.get(cursor)?, *body.get(cursor + 1)? as usize);
        let value = body.get(cursor + 2..cursor + 2 + size)?;
        match code { 0xC1 => local = Some(value), 0xC2 => remote = Some(value), _ => {} }
        cursor += 2 + size;
    }
    let (local, remote) = (local.filter(|v| v.len() == 2)?, remote.filter(|v| v.len() == 2)?);
    let mut tail = vec![0xC0, 0x01, 0x0A, 0xC1, 0x02];
    tail.extend_from_slice(remote);
    tail.extend_from_slice(&[0xC2, 0x02]);
    tail.extend_from_slice(local);
    let mut cotp = vec![(6 + tail.len()) as u8, 0xD0, body[4], body[5], 0x00, 0x01, 0x00];
    cotp.extend_from_slice(&tail);
    Some(tpkt(&cotp))
}

fn ack(reference: [u8; 2], parameters: &[u8], data: &[u8], error: [u8; 2]) -> Vec<u8> {
    let mut payload = vec![0x02, 0xF0, 0x80, 0x32, 0x03, 0x00, 0x00, reference[0], reference[1]];
    payload.extend_from_slice(&(parameters.len() as u16).to_be_bytes());
    payload.extend_from_slice(&(data.len() as u16).to_be_bytes());
    payload.extend_from_slice(&error);
    payload.extend_from_slice(parameters);
    payload.extend_from_slice(data);
    tpkt(&payload)
}

#[derive(Clone, Copy)]
struct Item {
    transport: u8,
    count: u16,
    db: u16,
    area: u8,
    address: u32,
}

impl Item {
    fn byte(&self) -> u32 { self.address / 8 }

    fn bit(&self) -> u8 { (self.address % 8) as u8 }

    fn validate(&self) -> Result<(), u8> {
        if self.area != AREA_DB || !matches!(self.transport, 1 | 2) { return Err(0x06); }
        if self.count == 0 || (self.transport == 1 && self.count != 1) { return Err(0x07); }
        if self.transport == 2 && self.bit() != 0 { return Err(0x05); }
        Ok(())
    }

    fn size(&self) -> usize { if self.transport == 1 { 1 } else { self.count as usize } }
}

fn parse_items(parameters: &[u8]) -> Option<Vec<Item>> {
    let count = *parameters.get(1)? as usize;
    if !(1..=20).contains(&count) || parameters.len() != 2 + 12 * count { return None; }
    parameters[2..].chunks(12).map(|spec| {
        (spec[..3] == [0x12, 0x0A, 0x10]).then(|| Item {
            transport: spec[3],
            count: u16::from_be_bytes([spec[4], spec[5]]),
            db: u16::from_be_bytes([spec[6], spec[7]]),
            area: spec[8],
            address: u32::from_be_bytes([0, spec[9], spec[10], spec[11]]),
        })
    }).collect()
}

fn parse_write_data<'a>(items: &[Item], data: &'a [u8]) -> Option<Vec<(u8, u16, &'a [u8])>> {
    let mut cursor = 0;
    let mut values = Vec::with_capacity(items.len());
    for index in 0..items.len() {
        let head = data.get(cursor..cursor + 4)?;
        if head[0] != 0 { return None; }
        let (transport, bits) = (head[1], u16::from_be_bytes([head[2], head[3]]));
        let len = (bits as usize).div_ceil(8);
        cursor += 4;
        values.push((transport, bits, data.get(cursor..cursor + len)?));
        cursor += len;
        if len % 2 == 1 && index + 1 < items.len() {
            if data.get(cursor) != Some(&0) { return None; }
            cursor += 1;
        }
    }
    (cursor == data.len()).then_some(values)
}

fn read_item(memory: &Memory, item: &Item) -> Result<(u8, u16, Vec<u8>), u8> {
    item.validate()?;
    if item.transport == 1 {
        return Ok((0x03, 1, vec![memory.bit(item.db, item.byte(), item.bit())? as u8]));
    }
    Ok((0x04, item.count * 8, memory.read(item.db, item.byte(), item.size())?))
}

fn write_item(memory: &mut Memory, item: &Item, transport: u8, bits: u16, data: &[u8]) -> Result<(), u8> {
    item.validate()?;
    if item.transport == 1 {
        if transport != 0x03 { return Err(0x06); }
        if bits != 1 || data.len() != 1 || data[0] > 1 { return Err(0x07); }
        return memory.set_bit(item.db, item.byte(), item.bit(), data[0] == 1);
    }
    if transport != 0x04 { return Err(0x06); }
    if bits as usize != item.size() * 8 || data.len() != item.size() { return Err(0x07); }
    memory.write(item.db, item.byte(), data)
}

fn exchange(s7: &[u8], pdu: &mut Option<u16>, memory: &SharedMemory, control: &Control) -> Option<Vec<u8>> {
    if s7.len() < 10 || s7[..2] != [0x32, 0x01] || s7[2..4] != [0, 0] { return None; }
    let reference = [s7[4], s7[5]];
    let parameter_len = u16::from_be_bytes([s7[6], s7[7]]) as usize;
    let data_len = u16::from_be_bytes([s7[8], s7[9]]) as usize;
    if parameter_len == 0 || s7.len() != 10 + parameter_len + data_len { return None; }
    let (parameters, data) = s7[10..].split_at(parameter_len);
    let function = parameters[0];
    if function == 0xF0 {
        if parameters.len() != 8 || parameters[1] != 0 || !data.is_empty() { return Some(ack(reference, &[], &[], REJECTED)); }
        let proposed = u16::from_be_bytes([parameters[6], parameters[7]]);
        if proposed < 240 { return Some(ack(reference, &[], &[], REJECTED)); }
        let chosen = proposed.min(MAX_PDU);
        *pdu = Some(chosen);
        let [high, low] = chosen.to_be_bytes();
        return Some(ack(reference, &[0xF0, 0x00, 0x00, 0x01, 0x00, 0x01, high, low], &[], [0, 0]));
    }
    let Some(limit) = pdu.map(usize::from).filter(|limit| s7.len() <= *limit && matches!(function, 0x04 | 0x05)) else {
        return Some(ack(reference, &[], &[], REJECTED));
    };
    let Some(items) = parse_items(parameters) else { return Some(ack(reference, &[], &[], REJECTED)) };
    let mut output = Vec::new();
    if function == 0x04 {
        if !data.is_empty() { return Some(ack(reference, &[], &[], REJECTED)); }
        let rejected = Control::take(&control.reject_reads);
        let memory = memory.lock().unwrap();
        for (index, item) in items.iter().enumerate() {
            match if rejected { Err(0x03) } else { read_item(&memory, item) } {
                Ok((transport, bits, value)) => {
                    output.extend_from_slice(&[0xFF, transport]);
                    output.extend_from_slice(&bits.to_be_bytes());
                    output.extend_from_slice(&value);
                    if value.len() % 2 == 1 && index + 1 < items.len() { output.push(0); }
                }
                Err(code) => output.extend_from_slice(&[code, 0, 0, 0]),
            }
        }
    } else {
        let Some(values) = parse_write_data(&items, data) else { return Some(ack(reference, &[], &[], REJECTED)) };
        let rejected = Control::take(&control.reject_writes);
        let (targeted, disconnect) = control.take_target(&items);
        let mut memory = memory.lock().unwrap();
        for (item, (transport, bits, value)) in items.iter().zip(values) {
            output.push(match if rejected || targeted(item) { Err(0x03) } else { write_item(&mut memory, item, transport, bits, value) } {
                Ok(()) => 0xFF,
                Err(code) => code,
            });
        }
        if disconnect { return None; }
    }
    let response = ack(reference, &[function, items.len() as u8], &output, [0, 0]);
    Some(if response.len() - 7 > limit { ack(reference, &[], &[], TOO_LARGE) } else { response })
}
