//! 原图文件读写：样本集、示教资料、录制原图共用。读取前拒绝符号链接与 reparse point。

use std::borrow::Cow;
use std::io::{Read, Write};
use std::path::Path;

use crate::frame::BayerPattern;
use crate::vision::FrameImage;

pub(crate) fn checked_source_path(path: &Path) -> Result<(), String> {
    let absolute = if path.is_absolute() { path.to_path_buf() } else {
        std::env::current_dir().map_err(|e| e.to_string())?.join(path)
    };
    for component in absolute.ancestors() {
        let metadata = std::fs::symlink_metadata(component)
            .map_err(|e| format!("原图路径 {} 无法读取：{e}", component.display()))?;
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            if metadata.file_attributes() & 0x400 != 0 {
                return Err(format!("原图路径 {} 包含 reparse point，拒绝读取", component.display()));
            }
        }
        if metadata.file_type().is_symlink() {
            return Err(format!("原图路径 {} 包含符号链接，拒绝读取", component.display()));
        }
    }
    Ok(())
}

/// 8 位 PNM 头（P5 灰度 / P6 RGB，可含 `#` 注释）。彩色录制的原图是带 `# bayer XX` 注释的 P5：内容是原始 Bayer 马赛克，
/// XX 为 GenICam 相位名。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PnmInfo {
    pub width: u32,
    pub height: u32,
    /// 文件里每像素字节数：P5 为 1，P6 为 3
    pub file_channels: u8,
    pub bayer: Option<BayerPattern>,
    /// 像素数据起点
    pub data_offset: usize,
}

impl PnmInfo {
    /// 文件应有的总长度。
    pub fn file_len(&self) -> u64 {
        self.data_offset as u64
            + u64::from(self.width) * u64::from(self.height) * u64::from(self.file_channels)
    }

    /// 读回后的通道数：Bayer 马赛克去马赛克成 RGB。
    pub fn channels(&self) -> u8 {
        if self.bayer.is_some() {
            3
        } else {
            self.file_channels
        }
    }
}

/// 解析 8 位 P5 / P6 头；不是这两种格式或最大值不是 255 时为 None（交给 image 解码）。
pub fn pnm_info(bytes: &[u8]) -> Option<PnmInfo> {
    let file_channels = match bytes.get(..2)? {
        b"P5" => 1,
        b"P6" => 3,
        _ => return None,
    };
    let mut at = 2;
    let mut fields = [0u32; 3];
    let mut bayer = None;
    for field in &mut fields {
        loop {
            match *bytes.get(at)? {
                b'#' => {
                    let end = bytes[at..]
                        .iter()
                        .position(|&b| b == b'\n')
                        .map_or(bytes.len(), |n| at + n);
                    let comment = std::str::from_utf8(&bytes[at + 1..end]).ok()?.trim();
                    if let Some(name) = comment.strip_prefix("bayer") {
                        bayer = Some(BayerPattern::from_name(name.trim())?);
                    }
                    at = end;
                }
                b if b.is_ascii_whitespace() => at += 1,
                _ => break,
            }
        }
        let digits = bytes[at..]
            .iter()
            .take_while(|b| b.is_ascii_digit())
            .count();
        if digits == 0 || digits > 9 {
            return None;
        }
        *field = std::str::from_utf8(&bytes[at..at + digits])
            .ok()?
            .parse()
            .ok()?;
        at += digits;
    }
    let [width, height, max] = fields;
    if max != 255 || width == 0 || height == 0 || !bytes.get(at)?.is_ascii_whitespace() {
        return None;
    }
    if file_channels == 3 && bayer.is_some() {
        return None;
    }
    Some(PnmInfo {
        width,
        height,
        file_channels,
        bayer,
        data_offset: at + 1,
    })
}

const MAX_COMPRESSED_IMAGE_BYTES: u64 = 128 * 1024 * 1024;

pub fn compressed_suffix(bytes: &[u8]) -> &'static str {
    if bytes.starts_with(&[0x28, 0xb5, 0x2f, 0xfd]) {
        ".zst"
    } else if bytes.starts_with(&[0x1f, 0x8b]) {
        ".gz"
    } else {
        ""
    }
}

fn read_bounded(reader: &mut impl Read) -> Result<Vec<u8>, String> {
    let mut raw = Vec::new();
    reader
        .take(MAX_COMPRESSED_IMAGE_BYTES + 1)
        .read_to_end(&mut raw)
        .map_err(|e| format!("压缩原图解码失败：{e}"))?;
    if raw.len() as u64 > MAX_COMPRESSED_IMAGE_BYTES {
        return Err("压缩原图解压后超过 128 MiB".into());
    }
    Ok(raw)
}

pub fn uncompressed_bytes(bytes: &[u8]) -> Result<Cow<'_, [u8]>, String> {
    let (raw, remaining) = match compressed_suffix(bytes) {
        ".zst" => {
            if bytes.get(4).is_none_or(|descriptor| descriptor & 4 == 0) {
                return Err("压缩原图缺少校验和".into());
            }
            let mut decoder = zstd::stream::read::Decoder::with_buffer(bytes)
                .map_err(|e| format!("压缩原图解码失败：{e}"))?
                .single_frame();
            decoder.window_log_max(27).map_err(|e| e.to_string())?;
            let raw = read_bounded(&mut decoder)?;
            (raw, decoder.finish())
        }
        ".gz" => {
            let mut decoder = flate2::bufread::GzDecoder::new(bytes);
            let raw = read_bounded(&mut decoder)?;
            (raw, decoder.into_inner())
        }
        _ => return Ok(Cow::Borrowed(bytes)),
    };
    if !remaining.is_empty() {
        return Err("压缩原图包含多余数据".into());
    }
    let info = pnm_info(&raw).ok_or("压缩原图不是有效 PNM")?;
    if info.file_len() != raw.len() as u64 {
        return Err("压缩原图像素长度不符".into());
    }
    Ok(Cow::Owned(raw))
}

pub fn verify_pnm(path: &Path, width: u32, height: u32) -> Result<(), String> {
    checked_source_path(path)?;
    if !path.is_file() {
        return Err("原图路径不是普通文件".into());
    }
    let bytes = read_checked(path)?;
    let raw = uncompressed_bytes(&bytes)?;
    let info = pnm_info(&raw).ok_or("原图不是有效 PNM")?;
    if (info.width, info.height) != (width, height) || info.file_len() != raw.len() as u64 {
        return Err("原图尺寸或像素长度不符".into());
    }
    Ok(())
}

pub fn compress_lossless(bytes: &[u8]) -> Result<Vec<u8>, String> {
    let raw = uncompressed_bytes(bytes)?;
    let Some(info) = pnm_info(&raw) else {
        return Ok(raw.into_owned());
    };
    if info.file_len() != raw.len() as u64 {
        return Err("原图像素长度不符，不能压缩".into());
    }
    if raw.len() as u64 > MAX_COMPRESSED_IMAGE_BYTES {
        return Err("原图超过 128 MiB，不能压缩".into());
    }
    let mut encoder =
        zstd::stream::write::Encoder::new(Vec::new(), 1).map_err(|e| e.to_string())?;
    encoder.include_checksum(true).map_err(|e| e.to_string())?;
    encoder
        .set_pledged_src_size(Some(raw.len() as u64))
        .map_err(|e| e.to_string())?;
    encoder.write_all(&raw).map_err(|e| e.to_string())?;
    let compressed = encoder.finish().map_err(|e| e.to_string())?;
    Ok(if compressed.len() < raw.len() {
        compressed
    } else {
        raw.into_owned()
    })
}

/// 读取 PNM 头；压缩原图先完整校验并解压。
pub fn read_pnm_info(path: &Path) -> Result<Option<PnmInfo>, String> {
    use std::io::Read;
    let mut head = Vec::with_capacity(512);
    std::fs::File::open(path)
        .and_then(|f| f.take(512).read_to_end(&mut head))
        .map_err(|e| format!("读取 {} 失败：{e}", path.display()))?;
    if !compressed_suffix(&head).is_empty() {
        let bytes = read_checked(path)?;
        return Ok(pnm_info(&uncompressed_bytes(&bytes)?));
    }
    Ok(pnm_info(&head))
}

/// [`load`] 读回这张图的通道数：带 Bayer 注释的 P5 与 P6 为 3，其余为 1。
pub fn load_channels(path: &Path) -> Result<u8, String> {
    checked_source_path(path)?;
    Ok(read_pnm_info(path)?.map_or(1, |info| info.channels()))
}

/// 8 位 PNM 头文字：RGB 写 P6，灰度写 P5；`bayer` 给了写成带相位注释的 P5（内容是原始马赛克）。
pub fn pnm_header(width: u32, height: u32, channels: u8, bayer: Option<BayerPattern>) -> String {
    match (channels, bayer) {
        (_, Some(pattern)) => format!("P5\n# bayer {}\n{width} {height}\n255\n", pattern.name()),
        (3, None) => format!("P6\n{width} {height}\n255\n"),
        _ => format!("P5\n{width} {height}\n255\n"),
    }
}

fn decode_pnm(bytes: &[u8], info: PnmInfo) -> Result<FrameImage, String> {
    let need = info.file_len() as usize - info.data_offset;
    let data = bytes
        .get(info.data_offset..info.data_offset + need)
        .ok_or_else(|| {
            format!(
                "PNM 像素数据不足：{}×{} 需要 {need} 字节",
                info.width, info.height
            )
        })?;
    Ok(match (info.bayer, info.file_channels) {
        (Some(pattern), _) => {
            let mut rgb = Vec::new();
            crate::frame::bayer8_rgb_into(
                pattern,
                info.width as usize,
                info.height as usize,
                data,
                &mut rgb,
            )?;
            FrameImage::rgb(info.width, info.height, rgb)
        }
        (None, 3) => FrameImage::rgb(info.width, info.height, data.to_vec()),
        _ => FrameImage::new(info.width, info.height, data.to_vec()),
    })
}

fn read_checked(path: &Path) -> Result<Vec<u8>, String> {
    checked_source_path(path)?;
    std::fs::read(path).map_err(|e| format!("读取 {} 失败：{e}", path.display()))
}

/// 读原图（统一入口）：带 `# bayer` 注释的 P5（彩色录制）去马赛克成 RGB，P6 读成 RGB，普通 P5 是灰度；
/// 支持 zstd / gzip 无损封装的 PNM；其他格式（PNG、JPG 等）读成 8 位灰度，彩色 / 16 位图按亮度换算。
pub fn load(path: &Path) -> Result<FrameImage, String> {
    let bytes = read_checked(path)?;
    decode(&bytes).map_err(|e| format!("解码 {} 失败：{e}", path.display()))
}

/// 同 [`load`]，但 PNG、JPG 等格式保留颜色读成 RGB（彩色模式的样本集与图片导入用）。
pub fn load_color(path: &Path) -> Result<FrameImage, String> {
    let bytes = read_checked(path)?;
    decode_color(&bytes).map_err(|e| format!("解码 {} 失败：{e}", path.display()))
}

/// 按模式读：彩色模式见 [`load_color`]，灰度模式见 [`load`]。
pub fn load_as(path: &Path, color: bool) -> Result<FrameImage, String> {
    if color {
        load_color(path)
    } else {
        load(path)
    }
}

/// 按内容识别格式解码，规则同 [`load`]。
pub fn decode(bytes: &[u8]) -> Result<FrameImage, String> {
    let raw = uncompressed_bytes(bytes)?;
    let bytes = raw.as_ref();
    if let Some(info) = pnm_info(bytes) {
        return decode_pnm(bytes, info);
    }
    let img = image::load_from_memory(bytes).map_err(|e| e.to_string())?.into_luma8();
    let (width, height) = img.dimensions();
    Ok(FrameImage::new(width, height, img.into_raw()))
}

/// 按内容识别格式解码，规则同 [`load_color`]。
pub fn decode_color(bytes: &[u8]) -> Result<FrameImage, String> {
    let raw = uncompressed_bytes(bytes)?;
    let bytes = raw.as_ref();
    if let Some(info) = pnm_info(bytes) {
        return decode_pnm(bytes, info);
    }
    let img = image::load_from_memory(bytes)
        .map_err(|e| e.to_string())?
        .into_rgb8();
    let (width, height) = img.dimensions();
    Ok(FrameImage::rgb(width, height, img.into_raw()))
}

/// 存成 PNM：灰度 P5、RGB P6（扩展名不变，读入按魔数认）。帧录制用，不压缩、写得快。
pub fn save_pgm(path: &Path, img: &FrameImage) -> Result<(), String> {
    use std::io::Write;
    let mut f = std::io::BufWriter::new(std::fs::File::create(path).map_err(|e| e.to_string())?);
    f.write_all(pnm_header(img.width, img.height, img.channels, None).as_bytes())
        .map_err(|e| e.to_string())?;
    f.write_all(&img.pixels).map_err(|e| e.to_string())?;
    f.flush().map_err(|e| e.to_string())
}

/// 冻结示教原图：按编号只写一次（发布包按示教图编号认示教帧）。文件已在时逐字节比对，
/// 相同照常（同一圈重复采用）、不同报错，不覆盖。灰度写 P5、RGB 写 P6。
pub fn save_pgm_new(path: &Path, img: &FrameImage) -> Result<(), String> {
    let mut bytes = pnm_header(img.width, img.height, img.channels, None).into_bytes();
    bytes.extend_from_slice(&img.pixels);
    save_bytes_new(path, &bytes)
}

/// 按 [`save_pgm_new`] 的规则冻结一份已编码的原图字节（彩色录制原图原样复制）。
pub fn save_bytes_new(path: &Path, bytes: &[u8]) -> Result<(), String> {
    use std::io::Write;
    match std::fs::OpenOptions::new().write(true).create_new(true).open(path) {
        Ok(mut file) => {
            if let Err(e) = file.write_all(bytes).and_then(|_| file.sync_all()) {
                drop(file);
                let _ = std::fs::remove_file(path);
                return Err(format!("写示教原图 {} 失败：{e}", path.display()));
            }
            Ok(())
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            checked_source_path(path)?;
            let existing = std::fs::read(path)
                .map_err(|e| format!("读取已有示教原图 {} 失败：{e}", path.display()))?;
            if uncompressed_bytes(&existing)? == uncompressed_bytes(bytes)? {
                Ok(())
            } else {
                Err(format!(
                    "示教图编号 {} 已存在且内容不同，不能覆盖（示教帧按图编号认来源）",
                    path.file_stem().and_then(|s| s.to_str()).unwrap_or("?")
                ))
            }
        }
        Err(e) => Err(format!("写示教原图 {} 失败：{e}", path.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gzip(raw: &[u8]) -> Vec<u8> {
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(raw).unwrap();
        encoder.finish().unwrap()
    }

    #[test]
    fn compressed_teaching_images_preserve_pixels_channels_and_frozen_identity() {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "gluesight-image-codec-{}-{nanos}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).unwrap();
        for (index, (channels, bayer)) in [
            (1, None),
            (3, None),
            (1, Some("RG")),
            (1, Some("BG")),
            (1, Some("GR")),
            (1, Some("GB")),
        ]
        .into_iter()
        .enumerate()
        {
            let pattern = bayer.map(|name| BayerPattern::from_name(name).unwrap());
            let mut raw = pnm_header(32, 32, channels, pattern).into_bytes();
            raw.extend((0..32 * 32 * channels as usize).map(|i| ((i % 32) * 7) as u8));
            let encoded = compress_lossless(&raw).unwrap();
            assert_eq!(compressed_suffix(&encoded), ".zst");
            assert!(encoded.len() < raw.len());
            let expected = decode(&raw).unwrap();
            for (codec, bytes) in [
                ("zstd", encoded),
                ("gzip", gzip(&raw)),
                ("raw", raw.clone()),
            ] {
                assert_eq!(uncompressed_bytes(&bytes).unwrap().as_ref(), raw);
                let path = root.join(format!("{index}-{codec}.pgm"));
                save_bytes_new(&path, &bytes).unwrap();
                save_bytes_new(&path, &raw).unwrap();
                for actual in [load(&path).unwrap(), decode_color(&bytes).unwrap()] {
                    assert_eq!(
                        (actual.width, actual.height, actual.channels),
                        (32, 32, expected.channels)
                    );
                    assert_eq!(actual.pixels, expected.pixels);
                }
                assert_eq!(load_channels(&path).unwrap(), expected.channels);
                assert_eq!(read_pnm_info(&path).unwrap(), pnm_info(&raw));
                verify_pnm(&path, 32, 32).unwrap();
                assert!(verify_pnm(&path, 16, 64).is_err());
                let mut changed = raw.clone();
                let last = changed.len() - 1;
                changed[last] ^= 1;
                assert!(save_bytes_new(&path, &changed).is_err());
                let mut invalid = bytes;
                invalid.push(0);
                std::fs::write(&path, invalid).unwrap();
                assert!(verify_pnm(&path, 32, 32).is_err());
            }
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn damaged_compressed_teaching_image_is_rejected_before_measurement() {
        let mut raw = pnm_header(32, 32, 1, None).into_bytes();
        raw.extend([42; 32 * 32]);
        let without_checksum = zstd::stream::encode_all(raw.as_slice(), 1).unwrap();
        assert!(decode(&without_checksum).is_err());
        for encoded in [compress_lossless(&raw).unwrap(), gzip(&raw)] {
            let mut bad_checksum = encoded.clone();
            let checksum = bad_checksum.len()
                - if compressed_suffix(&encoded) == ".gz" {
                    8
                } else {
                    4
                };
            bad_checksum[checksum] ^= 1;
            let truncated = encoded[..encoded.len() - 1].to_vec();
            let mut trailing = encoded.clone();
            trailing.push(0);
            let mut concatenated = encoded.clone();
            concatenated.extend_from_slice(&encoded);
            for invalid in [bad_checksum, truncated, trailing, concatenated] {
                assert!(decode(&invalid).is_err());
                assert!(compress_lossless(&invalid).is_err());
            }
        }
    }

    #[test]
    fn frozen_image_id_rewritten_with_different_pixels_is_rejected_and_same_pixels_pass() {
        // 防的回归：冻结图按编号覆盖写（同一圈重复采用、编号碰撞）后，示教图编号没变、像素却变了，
        // 按编号引用示教帧的走廊示教与发布包就不再能证明用的是同一帧
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let root = std::env::temp_dir().join(format!("gluesight-frozen-once-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("capture-1-0-v1.pgm");
        save_pgm_new(&path, &FrameImage::new(2, 1, vec![10, 20])).unwrap();
        save_pgm_new(&path, &FrameImage::new(2, 1, vec![10, 20])).unwrap();
        assert!(save_pgm_new(&path, &FrameImage::new(2, 1, vec![10, 21])).is_err());
        assert_eq!(load(&path).unwrap().pixels, [10, 20]);
        let _ = std::fs::remove_dir_all(&root);
    }
}
