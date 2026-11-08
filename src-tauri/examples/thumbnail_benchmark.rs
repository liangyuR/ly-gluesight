//! 本机只读性能桥接：先把已完成轮次复制/硬链接到隔离目录，再传 --root；不得指向生产资料。
//! cargo run --locked --example thumbnail_benchmark -- --root output/thumbnail-benchmark/captures --port 18777
use gluesight_lib::probe_api::{
    downsample, load_image, thumb_strip, FrameImage, RecipeCaptureHost,
};
use serde_json::json;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

fn decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::new();
    let mut at = 0;
    while at < bytes.len() {
        if bytes[at] == b'%' && at + 2 < bytes.len() {
            if let Ok(byte) = u8::from_str_radix(&value[at + 1..at + 3], 16) {
                out.push(byte);
                at += 3;
                continue;
            }
        }
        out.push(bytes[at]);
        at += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn request(
    host: &RecipeCaptureHost,
    route: &str,
    query: &HashMap<String, String>,
) -> Result<(Vec<u8>, &'static str, usize), String> {
    let arg = |key: &str| {
        query
            .get(key)
            .map(String::as_str)
            .ok_or_else(|| format!("missing {key}"))
    };
    if route == "/health" {
        return Ok((
            json!({"pid": std::process::id()}).to_string().into_bytes(),
            "application/json",
            0,
        ));
    }
    let round = arg("round")?;
    if route == "/meta" {
        return Ok((
            serde_json::to_vec(&host.get_burst(round)?).map_err(|e| e.to_string())?,
            "application/json",
            0,
        ));
    }
    let camera = arg("camera")?;
    if route == "/frame" {
        let seq = arg("seq")?.parse::<u64>().map_err(|e| e.to_string())?;
        let image = load_image(&host.burst_frame_path(round, camera, seq)?)?;
        let mut bytes = Vec::with_capacity(12 + image.pixels.len());
        for n in [image.width, image.height, u32::from(image.channels)] {
            bytes.extend(n.to_le_bytes());
        }
        bytes.extend_from_slice(&image.pixels);
        return Ok((bytes, "application/octet-stream", 1));
    }
    if route != "/thumbs" {
        return Err("unknown endpoint".into());
    }
    let mut reads = 0;
    let bytes = if arg("mode")? == "before" {
        let index = host.burst_frame_index(round)?;
        let frames: Vec<Option<FrameImage>> = (1..=index.round().group_count)
            .map(|seq| {
                index
                    .path(camera, seq)
                    .ok()
                    .and_then(|path| {
                        reads += 1;
                        load_image(&path).ok()
                    })
                    .map(|image| downsample(&image, 8))
            })
            .collect();
        thumb_strip(&frames)
    } else {
        let start = arg("start")?.parse::<u64>().map_err(|e| e.to_string())?;
        let count = arg("count")?.parse::<u32>().map_err(|e| e.to_string())?;
        host.burst_frame_thumbs(round, camera, start, count, |path| {
            reads += 1;
            load_image(path)
        })?
    };
    Ok((bytes, "application/octet-stream", reads))
}

fn serve(mut stream: TcpStream, host: Arc<RecipeCaptureHost>) {
    let mut bytes = Vec::new();
    let mut block = [0u8; 4096];
    while bytes.len() < 16384 {
        let Ok(count) = stream.read(&mut block) else {
            return;
        };
        if count == 0 {
            return;
        }
        bytes.extend_from_slice(&block[..count]);
        if bytes.windows(4).any(|part| part == b"\r\n\r\n") {
            break;
        }
    }
    let text = String::from_utf8_lossy(&bytes);
    let Some(target) = text
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
    else {
        return;
    };
    let (route, raw) = target.split_once('?').unwrap_or((target, ""));
    let query: HashMap<_, _> = raw
        .split('&')
        .filter_map(|part| part.split_once('='))
        .map(|(key, value)| (decode(key), decode(value)))
        .collect();
    let started = Instant::now();
    let result = request(&host, route, &query);
    let elapsed = started.elapsed().as_secs_f64() * 1000.0;
    let (status, body, content_type, reads) = match result {
        Ok((body, kind, reads)) => ("200 OK", body, kind, reads),
        Err(error) => (
            "400 Bad Request",
            error.into_bytes(),
            "text/plain; charset=utf-8",
            0,
        ),
    };
    let headers = format!("HTTP/1.1 {status}\r\nContent-Length: {}\r\nContent-Type: {content_type}\r\nAccess-Control-Allow-Origin: *\r\nAccess-Control-Expose-Headers: *\r\nCache-Control: no-store\r\nConnection: close\r\nX-Elapsed-Ms: {elapsed:.3}\r\nX-Image-Reads: {reads}\r\n\r\n", body.len());
    let _ = stream
        .write_all(headers.as_bytes())
        .and_then(|_| stream.write_all(&body));
    println!(
        "{}",
        json!({"route":route,"mode":query.get("mode"),"round":query.get("round"),"camera":query.get("camera"),"start":query.get("start"),"count":query.get("count"),"ms":elapsed,"images":reads,"bytes":body.len(),"status":status})
    );
}

fn main() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let option = |name| {
        args.iter()
            .position(|v| v == name)
            .and_then(|at| args.get(at + 1))
            .cloned()
    };
    let root =
        PathBuf::from(option("--root").ok_or("--root requires an isolated capture directory")?);
    if !root.is_dir() {
        return Err("capture directory does not exist".into());
    }
    let port = option("--port").unwrap_or_else(|| "18777".into());
    let host = RecipeCaptureHost::open(root)?;
    let listener = TcpListener::bind(format!("127.0.0.1:{port}")).map_err(|e| e.to_string())?;
    println!(
        "{}",
        json!({"ready":true,"pid":std::process::id(),"port":port})
    );
    for stream in listener.incoming() {
        let stream = stream.map_err(|e| e.to_string())?;
        let host = host.clone();
        std::thread::spawn(move || serve(stream, host));
    }
    Ok(())
}
