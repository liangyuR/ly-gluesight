use gluesight_production_simulator::{
    camera::CameraServer,
    config::{init_demo, Config},
    plc::{production, Plc},
    s7_server::{BitAddr, S7Server},
};
use ly_plc::protocol::{s7::S7, AreaKey, Location, Protocol, Region, RegionData, Unit, WriteValue};
use ly_plc::{ConnectionConfig, ProtocolKind};
use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, AtomicU64},
        Arc,
    },
    time::{Duration, Instant},
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::task::JoinHandle;

const FRAMES: usize = 3;

fn area(db: u16) -> AreaKey {
    AreaKey {
        code: 0x84,
        index: db,
        unit: Unit::Byte,
    }
}

async fn write(client: &mut S7, start: u32, bytes: Vec<u8>) {
    client
        .write(
            &Location {
                area: area(2),
                start,
                len: bytes.len() as u32,
                bit: None,
                writable: true,
            },
            &WriteValue::Bytes(bytes),
        )
        .await
        .unwrap();
}

async fn read(client: &mut S7) -> Vec<u8> {
    match client
        .read(&Region {
            area: area(4),
            start: 0,
            count: 68,
        })
        .await
        .unwrap()
    {
        RegionData::Bytes(bytes) => bytes,
        _ => panic!("S7 DB byte data required"),
    }
}

fn demo(tag: &str, lead_ms: u64, timeout_ms: u64) -> (PathBuf, Config) {
    let root = std::env::temp_dir().join(format!(
        "gs-simulator-contract-{}-{}-{tag}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    init_demo(&root).unwrap();
    let mut config = Config::load(&root.join("simulator.json")).unwrap();
    config.pass_frames = vec![FRAMES];
    config.capture_lead_ms = lead_ms;
    config.result_timeout_ms = timeout_ms;
    config.interval_ms = 0;
    for camera in &mut config.cameras {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        camera.port = listener.local_addr().unwrap().port();
        camera.frames.truncate(FRAMES);
    }
    (root, config)
}

struct Line {
    root: PathBuf,
    epoch: Instant,
    plc: Arc<Plc>,
    cameras: Arc<Vec<CameraServer>>,
    consumers: Vec<JoinHandle<Vec<(u64, u64)>>>,
}

async fn line(tag: &str, lead_ms: u64, timeout_ms: u64, parts: usize) -> Line {
    let (root, config) = demo(tag, lead_ms, timeout_ms);
    let epoch = Instant::now();
    let plc = Plc::new(config.clone(), epoch);
    let mut cameras = Vec::new();
    let mut consumers = Vec::new();
    let (ready, mut previews) = tokio::sync::mpsc::channel(config.cameras.len());
    for camera in &config.cameras {
        cameras.push(CameraServer::start(camera.clone(), epoch).await.unwrap());
        let mut socket = tokio::net::TcpStream::connect(("127.0.0.1", camera.port))
            .await
            .unwrap();
        socket
            .write_all(b"{\"command\":\"trigger\"}\n")
            .await
            .unwrap();
        let ready = ready.clone();
        consumers.push(tokio::spawn(async move {
            let mut frames = Vec::new();
            loop {
                let mut prefix = [0u8; 4];
                socket.read_exact(&mut prefix).await.unwrap();
                let mut bytes = vec![0u8; u32::from_be_bytes(prefix) as usize];
                socket.read_exact(&mut bytes).await.unwrap();
                let header: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
                let mut pixels = vec![0; header["payloadLength"].as_u64().unwrap() as usize];
                socket.read_exact(&mut pixels).await.unwrap();
                if pixels.is_empty() {
                    continue;
                }
                assert_eq!(
                    (header["width"].as_u64(), header["height"].as_u64()),
                    (Some(640), Some(480))
                );
                assert_eq!(pixels[100 * 640 + 320], 220);
                let number = header["frameNumber"].as_u64().unwrap();
                if number == 1 {
                    assert_eq!(header["gateHigh"], false);
                    ready.send(()).await.unwrap();
                } else {
                    assert_eq!(header["gateHigh"], true);
                    frames.push((number, header["timestampNs"].as_u64().unwrap()));
                    if frames.len() == FRAMES * parts {
                        return frames;
                    }
                }
            }
        }));
    }
    for _ in 0..config.cameras.len() {
        previews.recv().await.unwrap();
    }
    Line {
        root,
        epoch,
        plc,
        cameras: Arc::new(cameras),
        consumers,
    }
}

impl Line {
    async fn s7(&self) -> S7Server {
        S7Server::start(
            self.plc.memory.clone(),
            Some((
                BitAddr {
                    db: 4,
                    byte: 3,
                    bit: 0,
                },
                Duration::from_millis(100),
            )),
        )
        .await
        .unwrap()
    }

    fn published(&self) -> Vec<u8> {
        self.plc.memory.lock().unwrap().read(4, 0, 68).unwrap()
    }
}

struct Seen {
    version: u16,
    sequence: u32,
    product: u16,
    sn: Vec<u8>,
    ended: Duration,
}

fn pc(
    port: u16,
    epoch: Instant,
    parts: usize,
    reply: fn(u32) -> (u32, u16, u16),
) -> JoinHandle<Vec<Seen>> {
    tokio::spawn(async move {
        let connection = ConnectionConfig {
            protocol: ProtocolKind::S7,
            host: "127.0.0.1".into(),
            port,
            timeout_ms: 2000,
            ..ConnectionConfig::default()
        };
        let mut client = S7::connect(&connection).await.unwrap();
        write(&mut client, 0, 9u16.to_be_bytes().to_vec()).await;
        let start = Instant::now();
        let mut last_heartbeat = None;
        let mut open: Option<Seen> = None;
        let mut seen = Vec::new();
        while seen.len() < parts {
            let heartbeat = ((start.elapsed().as_millis() / 100) % 2) as u16;
            if last_heartbeat != Some(heartbeat) {
                write(&mut client, 2, heartbeat.to_be_bytes().to_vec()).await;
                last_heartbeat = Some(heartbeat);
            }
            let data = read(&mut client).await;
            if data[4] & 1 == 1 {
                if open.is_none() {
                    open = Some(Seen {
                        version: u16::from_be_bytes([data[0], data[1]]),
                        sequence: u32::from_be_bytes(data[8..12].try_into().unwrap()),
                        product: u16::from_be_bytes([data[12], data[13]]),
                        sn: data[18..18 + data[17] as usize].to_vec(),
                        ended: Duration::ZERO,
                    });
                }
            } else if let Some(mut part) = open.take() {
                part.ended = epoch.elapsed();
                let (result_sequence, result, fault) = reply(part.sequence);
                let mut body = vec![0u8; 56];
                body[0..2].copy_from_slice(&result.to_be_bytes());
                body[2..4].copy_from_slice(&fault.to_be_bytes());
                body[4] = 50;
                body[5] = part.sn.len() as u8;
                body[6..6 + part.sn.len()].copy_from_slice(&part.sn);
                write(&mut client, 12, body).await;
                write(&mut client, 8, result_sequence.to_be_bytes().to_vec()).await;
                seen.push(part);
            }
            assert!(
                start.elapsed() < Duration::from_secs(15),
                "PC 只等到 {} 件",
                seen.len()
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        seen
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn result_with_the_parts_sequence_after_part_done_is_taken() {
    let line = line("taken", 2000, 5000, 1).await;
    let s7 = line.s7().await;
    let pc = pc(s7.port(), line.epoch, 1, |sequence| (sequence, 90, 95));
    let started = line.epoch.elapsed();
    let outcome = tokio::time::timeout(
        Duration::from_secs(10),
        line.plc
            .run(1, "SIM-0000000001".into(), line.cameras.as_slice()),
    )
    .await
    .unwrap()
    .unwrap()
    .expect("序号一致的结果应被取走");
    assert_eq!(
        (outcome.sequence, outcome.result, outcome.fault),
        (1, 90, 95)
    );
    assert!(outcome.sn_echo_matches && !outcome.maintenance);
    let seen = pc.await.unwrap();
    assert_eq!(
        (
            seen[0].version,
            seen[0].sequence,
            seen[0].product,
            seen[0].sn.as_slice()
        ),
        (9, 1, 1, b"SIM-0000000001".as_slice())
    );
    let mut timestamps = None;
    for consumer in line.consumers {
        let frames = consumer.await.unwrap();
        assert_eq!(
            frames.iter().map(|f| f.0).collect::<Vec<_>>(),
            vec![2, 3, 4]
        );
        let actual: Vec<_> = frames.iter().map(|f| f.1).collect();
        assert!(actual[0] >= (started + Duration::from_millis(2000)).as_nanos() as u64);
        assert!(actual[FRAMES - 1] < seen[0].ended.as_nanos() as u64);
        if let Some(expected) = &timestamps {
            assert_eq!(&actual, expected);
        } else {
            timestamps = Some(actual);
        }
    }
    drop(s7);
    std::fs::remove_dir_all(line.root).unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn result_for_another_sequence_leaves_part_uninspected_and_next_part_proceeds() {
    let line = line("mismatch", 200, 500, 2).await;
    let s7 = line.s7().await;
    let pc = pc(s7.port(), line.epoch, 2, |sequence| {
        (if sequence == 1 { 1001 } else { sequence }, 11, 0)
    });
    let first = line
        .plc
        .run(1, "SIM-0000000001".into(), line.cameras.as_slice())
        .await
        .unwrap();
    assert!(first.is_none(), "{first:?}");
    let second = tokio::time::timeout(
        Duration::from_secs(10),
        line.plc
            .run(2, "SIM-0000000002".into(), line.cameras.as_slice()),
    )
    .await
    .unwrap()
    .unwrap()
    .expect("上一件未检后，下一件的结果仍应被取走");
    assert_eq!((second.sequence, second.result), (2, 11));
    assert_eq!(
        pc.await
            .unwrap()
            .iter()
            .map(|part| part.sequence)
            .collect::<Vec<_>>(),
        vec![1, 2]
    );
    drop(s7);
    std::fs::remove_dir_all(line.root).unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pc_offline_with_stale_result_of_same_sequence_still_captures_and_leaves_part_uninspected()
{
    let line = line("offline", 200, 300, 1).await;
    {
        let mut memory = line.plc.memory.lock().unwrap();
        memory.write(2, 0, &9u16.to_be_bytes()).unwrap();
        memory.write(2, 8, &1u32.to_be_bytes()).unwrap();
        memory.write(2, 12, &1u16.to_be_bytes()).unwrap();
    }
    let batch = tokio::time::timeout(
        Duration::from_secs(10),
        production(
            line.plc.clone(),
            line.cameras.clone(),
            1,
            Arc::new(AtomicBool::new(false)),
            Arc::new(AtomicU64::new(0)),
        ),
    )
    .await
    .expect("PC 不在线时工件应照常进行");
    assert!(batch.is_err(), "心跳失活的 PC 留下的结果不应被取走");
    let published = line.published();
    assert_eq!(
        (
            u32::from_be_bytes(published[8..12].try_into().unwrap()),
            published[4] & 1
        ),
        (1, 0)
    );
    for consumer in line.consumers {
        assert_eq!(consumer.await.unwrap().len(), FRAMES);
    }
    std::fs::remove_dir_all(line.root).unwrap();
}

#[tokio::test]
async fn sequence_at_maximum_wraps_next_part_to_one() {
    let (root, config) = demo("wrap", 0, 0);
    let plc = Plc::new(config, Instant::now());
    let _ = production(
        plc.clone(),
        Arc::new(Vec::new()),
        1,
        Arc::new(AtomicBool::new(false)),
        Arc::new(AtomicU64::new(i32::MAX as u64)),
    )
    .await;
    let published = plc.memory.lock().unwrap().read(4, 8, 4).unwrap();
    assert_eq!(u32::from_be_bytes(published.try_into().unwrap()), 1);
    std::fs::remove_dir_all(root).unwrap();
}
