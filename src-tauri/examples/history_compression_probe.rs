use std::path::{Path, PathBuf};
use std::time::Instant;
use gluesight_lib::probe_api::{compress_image_lossless, image_pnm_info, save_recording_raw, uncompressed_image_bytes, FrameImage};

fn images(root: &Path, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
    for entry in std::fs::read_dir(root)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() { images(&entry.path(), out)?; }
        else if entry.file_type()?.is_file() && entry.path().extension().is_some_and(|e| e == "pgm") { out.push(entry.path()); }
    }
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.len() != 2 { return Err("usage: history_compression_probe <recordings-or-teaching-dir> <new-output-dir>".into()); }
    let target = Path::new(&args[1]);
    std::fs::create_dir(target)?;
    let mut paths = Vec::new();
    images(Path::new(&args[0]), &mut paths)?;
    paths.sort();
    if paths.is_empty() { return Err("no PGM frames found".into()); }
    let selected: Vec<_> = paths.iter().step_by((paths.len() / 512).max(1)).take(512).collect();
    let mut raw_bytes = 0u64;
    let mut stored_bytes = 0u64;
    let mut encode_seconds = 0.0;
    let mut writes_ms = Vec::new();
    for (index, path) in selected.iter().enumerate() {
        let file = std::fs::read(path)?;
        let raw = uncompressed_image_bytes(&file)?;
        let info = image_pnm_info(&raw).ok_or("invalid PNM")?;
        let image = if info.file_channels == 3 {
            FrameImage::rgb(info.width, info.height, raw[info.data_offset..].to_vec())
        } else {
            FrameImage::new(info.width, info.height, raw[info.data_offset..].to_vec())
        };
        let start = Instant::now();
        let encoded = compress_image_lossless(&raw)?;
        encode_seconds += start.elapsed().as_secs_f64();
        if uncompressed_image_bytes(&encoded)?.as_ref() != raw.as_ref() { return Err("codec changed original bytes".into()); }
        let destination = target.join(format!("cam1_s{index:06}.pgm"));
        let start = Instant::now();
        save_recording_raw(&destination, &image, info.bayer)?;
        writes_ms.push(start.elapsed().as_secs_f64() * 1000.0);
        let saved = std::fs::read(&destination)?;
        let recovered = uncompressed_image_bytes(&saved)?;
        let decoded = image_pnm_info(&recovered).ok_or("recorded image invalid")?;
        if (decoded.width, decoded.height, decoded.file_channels, decoded.bayer) != (info.width, info.height, info.file_channels, info.bayer)
            || recovered[decoded.data_offset..] != raw[info.data_offset..] {
            return Err("recording changed Bayer/pixel data".into());
        }
        raw_bytes += raw.len() as u64;
        stored_bytes += saved.len() as u64;
    }
    let write_seconds = writes_ms.iter().sum::<f64>() / 1000.0;
    writes_ms.sort_by(f64::total_cmp);
    let report = serde_json::json!({
        "frames": selected.len(), "rawBytes": raw_bytes, "storedBytes": stored_bytes,
        "savedPercent": (1.0-stored_bytes as f64/raw_bytes as f64)*100.0,
        "encodeMiBPerSecond": raw_bytes as f64/1048576.0/encode_seconds,
        "encodeAndSyncWriteMiBPerSecond": raw_bytes as f64/1048576.0/write_seconds,
        "writeP95Ms": writes_ms[(writes_ms.len()*95/100).min(writes_ms.len()-1)],
        "allPixelsAndBayerMetadataEqual": true
    });
    let report = serde_json::to_string_pretty(&report)?;
    std::fs::write(target.join("report.json"), &report)?;
    println!("{report}");
    Ok(())
}
