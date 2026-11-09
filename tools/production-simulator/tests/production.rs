use gluesight_production_simulator::{
    camera::CameraServer,
    config::{init_demo, Config},
    plc::{production, Plc},
    s7_server::{BitAddr, S7Server},
};
use ly_plc::protocol::{s7::S7, AreaKey, Location, Protocol, Region, RegionData, Unit, WriteValue};
use ly_plc::{ConnectionConfig, ProtocolKind};
use std::{
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn normal_production_confirms_matching_result_but_rejects_another_parts_sn() {
    for wrong_sn in [false, true] {
        let root = std::env::temp_dir().join(format!(
            "gs-simulator-contract-{}-{}-{wrong_sn}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        init_demo(&root).unwrap();
        let mut config = Config::load(&root.join("simulator.json")).unwrap();
        config.pass_frames = vec![3];
        config.interval_ms = 200;
        for camera in &mut config.cameras {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            camera.port = listener.local_addr().unwrap().port();
            camera.frames.truncate(3);
        }
        let epoch = Instant::now();
        let plc = Plc::new(config.clone(), epoch);
        let s7 = S7Server::start(
            plc.memory.clone(),
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
        .unwrap();
        let mut cameras = Vec::new();
        let mut consumers = Vec::new();
        let (ready, mut previews) = tokio::sync::mpsc::channel(3);
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
                        frames.push((
                            number,
                            header["timestampNs"].as_u64().unwrap(),
                            epoch.elapsed(),
                        ));
                        if frames.len() == 3 {
                            return frames;
                        }
                    }
                }
            }));
        }
        for _ in 0..3 {
            previews.recv().await.unwrap();
        }
        let cameras = Arc::new(cameras);
        let (monitor_plc, monitor_cameras) = (plc.clone(), cameras.clone());
        let monitor = tokio::spawn(async move {
            loop {
                monitor_plc.poll(&monitor_cameras);
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        });
        let finished = Arc::new(AtomicBool::new(false));
        let done = finished.clone();
        let port = s7.port();
        let client = tokio::spawn(async move {
            let connection = ConnectionConfig {
                protocol: ProtocolKind::S7,
                host: "127.0.0.1".into(),
                port,
                timeout_ms: 2000,
                ..ConnectionConfig::default()
            };
            let mut client = S7::connect(&connection).await.unwrap();
            write(&mut client, 0, 8u16.to_be_bytes().to_vec()).await;
            write(&mut client, 4, 10u16.to_be_bytes().to_vec()).await;
            let mut states = Vec::new();
            let start = Instant::now();
            let mut request_at = None;
            let mut last_heartbeat = None;
            loop {
                let heartbeat = ((start.elapsed().as_millis() / 100) % 2) as u16;
                if last_heartbeat != Some(heartbeat) {
                    write(&mut client, 2, heartbeat.to_be_bytes().to_vec()).await;
                    last_heartbeat = Some(heartbeat);
                }
                let data = read(&mut client).await;
                let state = u16::from_be_bytes([data[4], data[5]]);
                if states.last() != Some(&state) {
                    states.push(state);
                }
                if state == 10 && request_at.is_none() {
                    request_at = Some(epoch.elapsed());
                    assert_eq!(u32::from_be_bytes(data[8..12].try_into().unwrap()), 1);
                    assert_eq!(&data[18..18 + data[17] as usize], b"SIM-0000000001");
                }
                if state == 20 {
                    let mut response = vec![0u8; 68];
                    response[0..2].copy_from_slice(&8u16.to_be_bytes());
                    response[2..4].copy_from_slice(&heartbeat.to_be_bytes());
                    response[4..6].copy_from_slice(&30u16.to_be_bytes());
                    response[8..12].copy_from_slice(&1u32.to_be_bytes());
                    response[12..14].copy_from_slice(&1u16.to_be_bytes());
                    let sn: &[u8] = if wrong_sn {
                        b"WRONG"
                    } else {
                        b"SIM-0000000001"
                    };
                    response[16] = 50;
                    response[17] = sn.len() as u8;
                    response[18..18 + sn.len()].copy_from_slice(sn);
                    write(&mut client, 0, response).await;
                } else if state == 30 {
                    write(&mut client, 4, 10u16.to_be_bytes().to_vec()).await;
                }
                if done.load(Ordering::SeqCst) {
                    return (
                        states,
                        request_at.unwrap(),
                        state,
                        u16::from_be_bytes([data[6], data[7]]),
                    );
                }
                assert!(
                    start.elapsed() < Duration::from_secs(15),
                    "production handshake timed out: {states:?}"
                );
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        });
        let until = Instant::now() + Duration::from_secs(5);
        while !plc.ready() {
            assert!(Instant::now() < until);
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let outcome = tokio::time::timeout(
            Duration::from_secs(10),
            production(
                plc.clone(),
                cameras.clone(),
                1,
                Arc::new(AtomicBool::new(false)),
                Arc::new(AtomicU64::new(0)),
            ),
        )
        .await
        .unwrap();
        finished.store(true, Ordering::SeqCst);
        let (states, request_at, last, reason) = client.await.unwrap();
        if wrong_sn {
            assert!(outcome.is_err());
            assert_eq!((last, reason), (90, 5));
            assert!(!states.contains(&30));
        } else {
            assert!(outcome.is_ok(), "{outcome:?}");
            assert_eq!(last, 0);
            assert!(states.contains(&30));
        }
        assert!(states.contains(&10) && states.contains(&20));
        let mut timestamps = None;
        for consumer in consumers {
            let frames = consumer.await.unwrap();
            assert_eq!(
                frames.iter().map(|f| f.0).collect::<Vec<_>>(),
                vec![2, 3, 4]
            );
            assert!(frames[0].2 >= request_at + Duration::from_millis(1900));
            let actual: Vec<_> = frames.iter().map(|f| f.1).collect();
            if let Some(expected) = &timestamps {
                assert_eq!(&actual, expected);
            } else {
                timestamps = Some(actual);
            }
        }
        monitor.abort();
        drop(cameras);
        drop(s7);
        assert!(
            root.starts_with(std::env::temp_dir())
                && root
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with("gs-simulator-contract-")
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}
