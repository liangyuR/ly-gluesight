//! 录制原图的无损压缩文件 `.gsr`（docs/decisions/image-storage.md IS-2）：8 位 P5 / P6 原图逐字节可还原。
//!
//! 布局（小端）：
//!
//! | 偏移 | 长度 | 内容 |
//! | --- | --- | --- |
//! | 0 | 4 | 魔数 `GSR\x1a` |
//! | 4 | 1 | 版本，当前为 1 |
//! | 5 | 1 | 变换：0 不变换，1 按行对左邻同通道像素取差，2 拆四个同色平面后各自按行取差 |
//! | 6 | 2 | PNM 文件头长度 H |
//! | 8 | 4 | 像素字节数 N |
//! | 12 | 4 | 原始文件全部字节（文件头 + 像素）的 CRC-32 |
//! | 16 | H | 原 PNM 文件头的原字节（含 `# bayer XX` 注释） |
//! | 16 + H | 到文件尾 | 一个 Zstandard 帧（level 1，带内容校验和），解压后是变换过的 N 字节像素 |

use super::{pnm_info, PnmInfo};
use std::io::{BufRead, BufReader, Cursor};

pub const EXTENSION: &str = "gsr";

const MAGIC: [u8; 4] = *b"GSR\x1a";
const VERSION: u8 = 1;
const FIXED: usize = 16;
/// PNM 文件头上限，与 [`super::read_pnm_info`] 读的长度一致。
const MAX_HEADER: usize = 512;
/// 解压后的像素字节上限。
const MAX_PIXELS: usize = 256 << 20;
const ZSTD_MAGIC: [u8; 4] = [0x28, 0xb5, 0x2f, 0xfd];
const ZSTD_CHECKSUM_FLAG: u8 = 0x04;
const ZSTD_SINGLE_SEGMENT_FLAG: u8 = 0x20;
const BAYER_STREAM_WINDOW_LOG: u32 = 21;
const BAYER_STREAM_MIN_PIXELS: usize = 4 << 20;
const RESTORE_BLOCK_BYTES: usize = 1 << 20;
const LEVEL: i32 = 1;

const NONE: u8 = 0;
const DELTA: u8 = 1;
const BAYER_PLANES: u8 = 2;

/// 文件开头是不是 `.gsr` 的魔数。
pub fn is_gsr(bytes: &[u8]) -> bool {
    bytes.get(..4) == Some(&MAGIC)
}

/// 读固定头与 PNM 文件头需要的最多字节数。
pub const PREFIX: usize = FIXED + MAX_HEADER;

struct Container<'a> {
    transform: u8,
    info: PnmInfo,
    pixels: usize,
    crc: u32,
    header: &'a [u8],
    payload: &'a [u8],
}

fn container(bytes: &[u8]) -> Result<Container<'_>, String> {
    if bytes.len() < FIXED || !is_gsr(bytes) {
        return Err("不是压缩原图文件".into());
    }
    if bytes[4] != VERSION {
        return Err(format!("压缩原图版本 {} 不受支持", bytes[4]));
    }
    let transform = bytes[5];
    let header_len = usize::from(u16::from_le_bytes([bytes[6], bytes[7]]));
    let pixels = u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]) as usize;
    let crc = u32::from_le_bytes([bytes[12], bytes[13], bytes[14], bytes[15]]);
    if header_len == 0 || header_len > MAX_HEADER || pixels > MAX_PIXELS {
        return Err("压缩原图的头或像素长度超出上限".into());
    }
    let header = bytes
        .get(FIXED..FIXED + header_len)
        .ok_or("压缩原图被截断")?;
    let info = pnm_info(header)
        .filter(|info| {
            info.data_offset == header_len && info.file_len() == (header_len + pixels) as u64
        })
        .ok_or("压缩原图的文件头与像素长度不一致")?;
    let planes = info.bayer.is_some() && info.width % 2 == 0 && info.height % 2 == 0;
    if transform > BAYER_PLANES || (transform == BAYER_PLANES && !planes) {
        return Err(format!("压缩原图的变换 {transform} 无效"));
    }
    Ok(Container {
        transform,
        info,
        pixels,
        crc,
        header,
        payload: &bytes[FIXED + header_len..],
    })
}

/// 只看文件开头（至少 [`PREFIX`] 字节或整个文件）：原图的 PNM 头信息与原始文件长度。
pub fn info(prefix: &[u8]) -> Result<(PnmInfo, u64), String> {
    let container = container(prefix)?;
    Ok((container.info, container.info.file_len()))
}

fn row_delta(pixels: &[u8], row: usize, stride: usize) -> Vec<u8> {
    let mut out = pixels.to_vec();
    for line in out.chunks_exact_mut(row) {
        for i in (stride..row).rev() {
            line[i] = line[i].wrapping_sub(line[i - stride]);
        }
    }
    out
}

fn row_restore(
    pixels: &mut [u8],
    row: usize,
    stride: usize,
    check: &mut impl FnMut() -> Result<(), String>,
) -> Result<(), String> {
    for line in pixels.chunks_exact_mut(row) {
        for start in (stride..row).step_by(RESTORE_BLOCK_BYTES) {
            check()?;
            for i in start..(start + RESTORE_BLOCK_BYTES).min(row) {
                line[i] = line[i].wrapping_add(line[i - stride]);
            }
        }
    }
    Ok(())
}

/// 四个同色平面依次为马赛克的 (0,0)、(0,1)、(1,0)、(1,1) 相位，每个平面按行对左邻取差。
fn planes_delta(pixels: &[u8], width: usize, height: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(pixels.len());
    for phase in 0..4 {
        let (dy, dx) = (phase / 2, phase % 2);
        for y in (dy..height).step_by(2) {
            let line = &pixels[y * width..(y + 1) * width];
            let mut previous = 0u8;
            for &value in line[dx..].iter().step_by(2) {
                out.push(value.wrapping_sub(previous));
                previous = value;
            }
        }
    }
    out
}

fn restore_plane_pixels(
    pixels: &mut [u8],
    deltas: &mut std::slice::Iter<'_, u8>,
    mut previous: u8,
    check: &mut impl FnMut() -> Result<(), String>,
) -> Result<u8, String> {
    for block in pixels.chunks_mut(RESTORE_BLOCK_BYTES) {
        check()?;
        for value in block.iter_mut().step_by(2) {
            previous = previous.wrapping_add(*deltas.next().unwrap_or(&0));
            *value = previous;
        }
    }
    Ok(previous)
}

fn planes_restore(
    planes: &[u8],
    out: &mut [u8],
    width: usize,
    height: usize,
    check: &mut impl FnMut() -> Result<(), String>,
) -> Result<(), String> {
    let mut source = planes.iter();
    for phase in 0..4 {
        let (dy, dx) = (phase / 2, phase % 2);
        for y in (dy..height).step_by(2) {
            restore_plane_pixels(
                &mut out[y * width + dx..(y + 1) * width],
                &mut source,
                0,
                check,
            )?;
        }
    }
    Ok(())
}

fn planes_restore_stream(
    mut source: impl BufRead,
    out: &mut [u8],
    width: usize,
    height: usize,
    check: &mut impl FnMut() -> Result<(), String>,
) -> Result<(), String> {
    let decompression_error = |error| format!("压缩原图解压失败：{error}");
    let bad_length = "压缩原图解压后的长度与文件头不符";
    for phase in 0..4 {
        let (dy, dx) = (phase / 2, phase % 2);
        for y in (dy..height).step_by(2) {
            let mut line = &mut out[y * width + dx..(y + 1) * width];
            let mut previous = 0u8;
            while !line.is_empty() {
                check()?;
                let buffer = source.fill_buf().map_err(decompression_error)?;
                let count = line.len().div_ceil(2).min(buffer.len());
                if count == 0 {
                    return Err(bad_length.into());
                }
                let (pixels, rest) = line.split_at_mut((2 * count).min(line.len()));
                previous =
                    restore_plane_pixels(pixels, &mut buffer[..count].iter(), previous, check)?;
                line = rest;
                source.consume(count);
            }
        }
    }
    check()?;
    if !source.fill_buf().map_err(decompression_error)?.is_empty() {
        return Err(bad_length.into());
    }
    Ok(())
}

fn can_stream_bayer(payload: &[u8], pixels: usize) -> bool {
    // RFC 8878：窗口描述字节按窗口大小单调递增，低三位为零时表示 2 的整数次幂。
    let window_limit = ((BAYER_STREAM_WINDOW_LOG - 10) << 3) as u8;
    pixels >= BAYER_STREAM_MIN_PIXELS
        && payload[4] & ZSTD_SINGLE_SEGMENT_FLAG == 0
        && payload.get(5).is_some_and(|&window| window <= window_limit)
}

fn decompress_exact(
    payload: &[u8],
    destination: &mut impl zstd::zstd_safe::WriteBuf,
    pixels: usize,
) -> Result<(), String> {
    let length = zstd::bulk::Decompressor::new()
        .and_then(|mut decoder| decoder.decompress_to_buffer(payload, destination))
        .map_err(|error| format!("压缩原图解压失败：{error}"))?;
    if length != pixels {
        return Err("压缩原图解压后的长度与文件头不符".into());
    }
    Ok(())
}

/// 把一张 8 位 P5 / P6 原图的全部字节编码成 `.gsr`；不是这两种格式、文件头过长或长度与头不符（有多余字节）时为 None，
/// 这样的文件不压缩。
pub fn encode(original: &[u8]) -> Option<Vec<u8>> {
    let info = pnm_info(original)?;
    if info.data_offset > MAX_HEADER || info.file_len() != original.len() as u64 {
        return None;
    }
    let (header, pixels) = original.split_at(info.data_offset);
    if pixels.len() > MAX_PIXELS {
        return None;
    }
    let (width, height) = (info.width as usize, info.height as usize);
    let (transform, transformed) = if info.bayer.is_some() && width % 2 == 0 && height % 2 == 0 {
        (BAYER_PLANES, planes_delta(pixels, width, height))
    } else {
        let stride = usize::from(info.file_channels);
        (DELTA, row_delta(pixels, width * stride, stride))
    };
    let mut compressor = zstd::bulk::Compressor::new(LEVEL).ok()?;
    compressor
        .set_parameter(zstd::zstd_safe::CParameter::ChecksumFlag(true))
        .ok()?;
    let payload = compressor.compress(&transformed).ok()?;
    let mut out = Vec::with_capacity(FIXED + header.len() + payload.len());
    out.extend_from_slice(&MAGIC);
    out.push(VERSION);
    out.push(transform);
    out.extend_from_slice(&(header.len() as u16).to_le_bytes());
    out.extend_from_slice(&(pixels.len() as u32).to_le_bytes());
    out.extend_from_slice(&crc32fast::hash(original).to_le_bytes());
    out.extend_from_slice(header);
    out.extend_from_slice(&payload);
    Some(out)
}

/// 还原出原始 PNM 文件的全部字节。负载必须正好是一个带校验和的 Zstandard 帧、解压后正好是头里记的像素字节数，
/// 整个文件的 CRC-32 也要对上；任何一项不符都报错，不返回像素。
pub fn decode(bytes: &[u8]) -> Result<Vec<u8>, String> {
    decode_checked(bytes, || Ok(()))
}

pub(super) fn decode_checked(
    bytes: &[u8],
    mut check: impl FnMut() -> Result<(), String>,
) -> Result<Vec<u8>, String> {
    check()?;
    let Container {
        transform,
        info,
        pixels,
        crc,
        header,
        payload,
    } = container(bytes)?;
    if payload.get(..4) != Some(&ZSTD_MAGIC)
        || payload
            .get(4)
            .is_none_or(|flags| flags & ZSTD_CHECKSUM_FLAG == 0)
    {
        return Err("压缩原图的负载不是带校验和的 Zstandard 帧".into());
    }
    if zstd::zstd_safe::find_frame_compressed_size(payload) != Ok(payload.len()) {
        return Err("压缩原图被截断或带有多余数据".into());
    }
    check()?;
    let (width, height) = (info.width as usize, info.height as usize);
    let mut out = Vec::with_capacity(header.len() + pixels);
    out.extend_from_slice(header);
    match transform {
        NONE | DELTA => {
            let mut destination = Cursor::new(out);
            destination.set_position(header.len() as u64);
            decompress_exact(payload, &mut destination, pixels)?;
            check()?;
            out = destination.into_inner();
            if transform == DELTA {
                let stride = usize::from(info.file_channels);
                row_restore(&mut out[header.len()..], width * stride, stride, &mut check)?;
            }
        }
        _ => {
            out.resize(header.len() + pixels, 0);
            check()?;
            let original = &mut out[header.len()..];
            if can_stream_bayer(payload, pixels) {
                let mut decoder = zstd::stream::read::Decoder::with_buffer(payload)
                    .map_err(|error| format!("压缩原图解压失败：{error}"))?
                    .single_frame();
                decoder
                    .window_log_max(BAYER_STREAM_WINDOW_LOG)
                    .map_err(|error| format!("压缩原图解压失败：{error}"))?;
                planes_restore_stream(
                    BufReader::with_capacity(128 << 10, decoder),
                    original,
                    width,
                    height,
                    &mut check,
                )?;
            } else {
                let mut planes = Vec::with_capacity(pixels);
                decompress_exact(payload, &mut planes, pixels)?;
                check()?;
                planes_restore(&planes, original, width, height, &mut check)?;
            }
        }
    }
    let mut hasher = crc32fast::Hasher::new();
    for block in out.chunks(RESTORE_BLOCK_BYTES) {
        check()?;
        hasher.update(block);
    }
    check()?;
    if hasher.finalize() != crc {
        return Err("压缩原图校验和不符".into());
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `header` 加 `pixels` 个伪随机像素的原图文件。
    fn raw(header: &str, pixels: usize) -> Vec<u8> {
        let mut bytes = header.as_bytes().to_vec();
        let mut state = 0x2545_f491u32;
        bytes.extend((0..pixels).map(|_| {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (state >> 24) as u8
        }));
        bytes
    }

    #[test]
    fn every_recorded_format_restores_byte_for_byte() {
        // 防的回归：压缩再还原后与录制写下的文件差了哪怕一个字节（文件头注释、相位、像素），复测与示教用的就不是原图
        let cases = [
            ("灰度", "P5\n7 5\n255\n", 35, DELTA),
            ("RGB", "P6\n6 4\n255\n", 72, DELTA),
            ("Bayer BG", "P5\n# bayer BG\n8 6\n255\n", 48, BAYER_PLANES),
            ("Bayer RG", "P5\n# bayer RG\n8 6\n255\n", 48, BAYER_PLANES),
            ("Bayer GB", "P5\n# bayer GB\n8 6\n255\n", 48, BAYER_PLANES),
            ("Bayer GR", "P5\n# bayer GR\n8 6\n255\n", 48, BAYER_PLANES),
            ("奇数宽的 Bayer", "P5\n# bayer RG\n7 6\n255\n", 42, DELTA),
            ("奇数高的 Bayer", "P5\n# bayer RG\n8 5\n255\n", 40, DELTA),
            (
                "另有注释",
                "P5\n# note\n# bayer GR\n4 4\n255\n",
                16,
                BAYER_PLANES,
            ),
        ];
        for (name, header, pixels, transform) in cases {
            let original = raw(header, pixels);
            let encoded = encode(&original).unwrap_or_else(|| panic!("{name} 应能压缩"));
            assert_eq!(encoded[5], transform, "{name}");
            assert_eq!(decode(&encoded).unwrap(), original, "{name}");
        }
        let header = "P5\n# legacy\n7 5\n255\n";
        let original = raw(header, 35);
        let mut encoded = encode(&original).unwrap();
        encoded[5] = NONE;
        encoded.truncate(FIXED + header.len());
        let mut compressor = zstd::bulk::Compressor::new(LEVEL).unwrap();
        compressor
            .set_parameter(zstd::zstd_safe::CParameter::ChecksumFlag(true))
            .unwrap();
        encoded.extend(compressor.compress(&original[header.len()..]).unwrap());
        assert_eq!(decode(&encoded).unwrap(), original);
    }

    #[test]
    fn file_stored_in_format_version_1_still_restores() {
        // 防的回归：改了固定头布局或变换后，盘上已有的 .gsr 还原不出原图。同一份样本也是 scripts/gsr_raw.py 的回归输入
        // （tests/native/gsr-raw_test.py）
        let bytes = |hex: &str| -> Vec<u8> {
            (0..hex.len())
                .step_by(2)
                .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
                .collect()
        };
        let original = bytes(concat!(
            "50350a232062617965722052470a3820360a3235350a",
            "002f5e1544732a595d1443722958873e42712857863d6c9b2756853c6b9a5180843b6a99507fae6569984f7ead6493c2"
        ));
        let stored = bytes(concat!(
            "4753521a010216003000000088815a34",
            "50350a232062617965722052470a3820360a3235350a",
            "28b52ffd2430810100005ee6e642e65ee684e6e65e2fe65ee671e6e65e3b5ee6e65de6e65e275ee6e669e65ee6145ee6e656e65ee698e6e65e34e7b1a9"
        ));
        assert_eq!(decode(&stored).unwrap(), original);
    }

    #[test]
    fn large_bayer_restores_across_blocks_and_accepts_wider_legacy_windows() {
        let header = "P5\n# bayer RG\n2050 2048\n255\n";
        let original = raw(header, 2050 * 2048);
        let encoded = encode(&original).unwrap();
        assert_eq!(decode(&encoded).unwrap(), original);

        let descriptor = FIXED + header.len() + 4;
        assert_eq!(encoded[descriptor] & ZSTD_SINGLE_SEGMENT_FLAG, 0);
        let mut wider_window = encoded.clone();
        wider_window[descriptor + 1] = (23 - 10) << 3;
        assert_eq!(decode(&wider_window).unwrap(), original);

        let mut damaged = encoded;
        *damaged.last_mut().unwrap() ^= 1;
        assert!(decode(&damaged).is_err());
    }

    #[test]
    fn cancelling_during_gsr_pixel_restore_returns_no_partial_image() {
        for (header, wide_window) in [
            ("P5\n2048 2048\n255\n", false),
            ("P5\n# bayer RG\n2048 2048\n255\n", false),
            ("P5\n# bayer RG\n2048 2048\n255\n", true),
        ] {
            let original = raw(header, 2048 * 2048);
            let mut encoded = encode(&original).unwrap();
            if wide_window {
                let descriptor = FIXED + header.len() + 4;
                assert_eq!(encoded[descriptor] & ZSTD_SINGLE_SEGMENT_FLAG, 0);
                encoded[descriptor + 1] = (23 - 10) << 3;
            }
            let mut remaining = 32;
            let result = decode_checked(&encoded, || {
                if remaining == 0 {
                    return Err("cancel restore".into());
                }
                remaining -= 1;
                Ok(())
            });
            assert_eq!(result.err().as_deref(), Some("cancel restore"));
        }
    }

    #[test]
    fn file_that_could_not_be_restored_exactly_is_left_uncompressed() {
        // 防的回归：像素之后还有多余字节、不是 8 位 P5 / P6 的文件被压缩，还原时丢掉或改掉这些内容
        let mut longer = raw("P5\n4 4\n255\n", 16);
        longer.push(0);
        assert!(encode(&longer).is_none());
        assert!(encode(&raw("P5\n4 4\n65535\n", 32)).is_none());
        assert!(encode(&raw("P5\n4 4\n255\n", 15)).is_none());
    }

    #[test]
    fn damaged_or_oversized_compressed_file_is_rejected() {
        // 防的回归：损坏、截断、被改过或超限的压缩文件解出了像素，复测与示教拿到错图而不是读图失败
        const HEADER: &str = "P5\n# bayer RG\n16 16\n255\n";
        let original = raw(HEADER, 256);
        let good = encode(&original).unwrap();
        let payload = FIXED + HEADER.len();
        let edited = |edit: &dyn Fn(&mut Vec<u8>)| {
            let mut bytes = good.clone();
            edit(&mut bytes);
            bytes
        };
        let unchecked = {
            let mut bytes = good[..payload].to_vec();
            bytes.extend(
                zstd::bulk::compress(&planes_delta(&original[HEADER.len()..], 16, 16), LEVEL)
                    .unwrap(),
            );
            bytes
        };
        let huge = {
            let header = "P5\n20000 20000\n255\n";
            let mut bytes = MAGIC.to_vec();
            bytes.extend([VERSION, DELTA]);
            bytes.extend((header.len() as u16).to_le_bytes());
            bytes.extend(400_000_000u32.to_le_bytes());
            bytes.extend(0u32.to_le_bytes());
            bytes.extend(header.as_bytes());
            bytes.extend(&good[payload..]);
            bytes
        };
        let cases: Vec<(&str, Vec<u8>)> = vec![
            ("截断", good[..good.len() - 3].to_vec()),
            ("只剩固定头", good[..FIXED].to_vec()),
            ("负载被改", edited(&|b| b[payload + 12] ^= 0x10)),
            ("负载校验和被改", edited(&|b| *b.last_mut().unwrap() ^= 1)),
            (
                "文件头被改",
                edited(&|b| b[FIXED + 11..FIXED + 13].copy_from_slice(b"GR")),
            ),
            ("尾随数据", edited(&|b| b.push(0))),
            ("尾随第二个帧", edited(&|b| b.extend_from_within(payload..))),
            ("负载没有校验和", unchecked),
            ("像素长度超限", huge),
            ("像素长度与文件头不符", edited(&|b| b[8] ^= 1)),
            ("版本不认识", edited(&|b| b[4] = 2)),
            ("变换不认识", edited(&|b| b[5] = 9)),
            ("变换与文件头不符", edited(&|b| b[5] = DELTA)),
        ];
        assert_eq!(decode(&good).unwrap(), original);
        for (name, bytes) in cases {
            assert!(decode(&bytes).is_err(), "{name}");
        }
    }
}
