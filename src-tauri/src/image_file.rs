//! 原图文件读写：样本集、示教资料、录制原图共用。读取前拒绝符号链接与 reparse point。
//!
//! 录制原图 `X.pgm` 空闲压缩后是同目录同主名的 `X.gsr`（[`gsr`]），记录里的文件名不变：这里的读取入口在 `X.pgm` 不在时读
//! `X.gsr` 并还原出原始字节，两者都在时用 `X.pgm`。

use std::path::{Path, PathBuf};

pub mod gsr;

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

/// 录制原图 `X.pgm` 压缩后的文件 `X.gsr`；`path` 不是 `.pgm` 时为 None。
pub fn compressed_sibling(path: &Path) -> Option<PathBuf> {
    path.extension()
        .is_some_and(|extension| extension == "pgm")
        .then(|| path.with_extension(gsr::EXTENSION))
}

/// 原图实际在盘上的文件：`path` 在就是它；不在而同名的 `.gsr` 在，就是那个 `.gsr`。都不在为 None。
pub fn stored_path(path: &Path) -> Option<PathBuf> {
    if path.is_file() {
        return Some(path.to_path_buf());
    }
    compressed_sibling(path).filter(|sibling| sibling.is_file())
}

/// 原图在不在（未压缩或已压缩）。
pub fn exists(path: &Path) -> bool {
    stored_path(path).is_some()
}

fn is_compressed(path: &Path) -> bool {
    path.extension()
        .is_some_and(|extension| extension == gsr::EXTENSION)
}

/// `checked` 时先拒绝路径上的符号链接与 reparse point。
fn open_file(path: &Path, checked: bool) -> std::io::Result<std::fs::File> {
    if checked {
        checked_source_path(path).map_err(|error| {
            let kind = match std::fs::symlink_metadata(path) {
                Err(e) => e.kind(),
                Ok(_) => std::io::ErrorKind::PermissionDenied,
            };
            std::io::Error::new(kind, error)
        })?;
    }
    std::fs::File::open(path)
}

/// 打开原图实际在盘上的文件：`path` 本身，或它不在时同名的 `.gsr`（压缩线程在两者之间替换时总有一个在）。
fn open_stored(path: &Path, checked: bool) -> Result<(std::fs::File, bool), String> {
    let fail = |e: std::io::Error| format!("读取 {} 失败：{e}", path.display());
    match open_file(path, checked) {
        Ok(file) => Ok((file, is_compressed(path))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            match compressed_sibling(path).map(|sibling| open_file(&sibling, checked)) {
                Some(Ok(file)) => Ok((file, true)),
                Some(Err(e)) if e.kind() != std::io::ErrorKind::NotFound => Err(fail(e)),
                _ => Err(fail(error)),
            }
        }
        Err(error) => Err(fail(error)),
    }
}

/// 原图文件的原始字节（未压缩时的全部字节）：`path` 是 `.pgm` 而文件已压缩成同名 `.gsr`，或 `path` 本身就是 `.gsr` 时
/// 还原后返回；压缩文件损坏时报错。
pub fn read_original(path: &Path) -> Result<Vec<u8>, String> {
    use std::io::Read;
    crate::raw_compress::activity().touch();
    let (mut file, compressed) = open_stored(path, true)?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .map_err(|e| format!("读取 {} 失败：{e}", path.display()))?;
    if compressed {
        gsr::decode(&bytes).map_err(|e| format!("读取 {} 失败：{e}", path.display()))
    } else {
        Ok(bytes)
    }
}

/// 原图的 PNM 头与原始文件长度（未压缩时的字节数），只读文件开头；找文件的规则同 [`read_original`]，不查路径上的链接。
/// 不是 8 位 P5 / P6 时头为 None。
pub fn read_original_info(path: &Path) -> Result<(Option<PnmInfo>, u64), String> {
    use std::io::Read;
    let fail = |e: std::io::Error| format!("读取 {} 失败：{e}", path.display());
    let (file, compressed) = open_stored(path, false)?;
    let length = file.metadata().map_err(fail)?.len();
    let mut head = Vec::with_capacity(gsr::PREFIX);
    file.take(gsr::PREFIX as u64)
        .read_to_end(&mut head)
        .map_err(fail)?;
    if compressed {
        let (info, length) =
            gsr::info(&head).map_err(|e| format!("读取 {} 失败：{e}", path.display()))?;
        Ok((Some(info), length))
    } else {
        Ok((pnm_info(&head), length))
    }
}

/// 读文件开头解析 PNM 头（见 [`pnm_info`]）；压缩的原图读它记下的原文件头。
pub fn read_pnm_info(path: &Path) -> Result<Option<PnmInfo>, String> {
    read_original_info(path).map(|(info, _)| info)
}

/// [`load`] 读回这张图的通道数（只读文件头）：带 Bayer 注释的 P5 与 P6 为 3，其余为 1。
pub fn load_channels(path: &Path) -> Result<u8, String> {
    checked_source_path(&stored_path(path).unwrap_or_else(|| path.to_path_buf()))?;
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

/// 读原图（统一入口）：带 `# bayer` 注释的 P5（彩色录制）去马赛克成 RGB，P6 读成 RGB，普通 P5 是灰度；
/// 其他格式（PNG、JPG 等）读成 8 位灰度，彩色 / 16 位图按亮度换算。
pub fn load(path: &Path) -> Result<FrameImage, String> {
    let bytes = read_original(path)?;
    decode(&bytes).map_err(|e| format!("解码 {} 失败：{e}", path.display()))
}

/// 同 [`load`]，但 PNG、JPG 等格式保留颜色读成 RGB（彩色模式的样本集与图片导入用）。
pub fn load_color(path: &Path) -> Result<FrameImage, String> {
    let bytes = read_original(path)?;
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
    if let Some(info) = pnm_info(bytes) {
        return decode_pnm(bytes, info);
    }
    let img = image::load_from_memory(bytes).map_err(|e| e.to_string())?.into_luma8();
    let (width, height) = img.dimensions();
    Ok(FrameImage::new(width, height, img.into_raw()))
}

/// 按内容识别格式解码，规则同 [`load_color`]。
pub fn decode_color(bytes: &[u8]) -> Result<FrameImage, String> {
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
            if existing == bytes {
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

    #[test]
    fn compressed_recording_reads_as_the_same_image_and_plain_pgm_wins_when_both_exist() {
        // 防的回归：原图空闲压缩成 .gsr 后，按记录里的 .pgm 文件名读不到图或读出别的像素；压缩文件损坏时返回了错图；
        // 压缩中断留下两个文件时没用未压缩的那份
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root =
            std::env::temp_dir().join(format!("gluesight-gsr-read-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("cam1_s000001.pgm");
        let raw = |seed: u8| {
            let mut bytes = pnm_header(8, 6, 1, Some(BayerPattern::Rg)).into_bytes();
            bytes.extend((0..48u8).map(|i| i.wrapping_mul(5) ^ seed));
            bytes
        };
        let original = raw(0);
        std::fs::write(&path, &original).unwrap();
        let plain = load(&path).unwrap();
        let info = read_original_info(&path).unwrap();

        std::fs::write(path.with_extension("gsr"), gsr::encode(&original).unwrap()).unwrap();
        std::fs::remove_file(&path).unwrap();
        assert!(exists(&path));
        assert_eq!(read_original(&path).unwrap(), original);
        assert_eq!(read_original_info(&path).unwrap(), info);
        let restored = load(&path).unwrap();
        assert_eq!(
            (
                restored.width,
                restored.height,
                restored.channels,
                &restored.pixels
            ),
            (plain.width, plain.height, plain.channels, &plain.pixels)
        );
        assert_eq!(load_channels(&path).unwrap(), 3);

        std::fs::write(&path, raw(7)).unwrap();
        assert_eq!(read_original(&path).unwrap(), raw(7));

        std::fs::remove_file(&path).unwrap();
        let mut damaged = gsr::encode(&original).unwrap();
        let last = damaged.len() - 1;
        damaged[last] ^= 1;
        std::fs::write(path.with_extension("gsr"), damaged).unwrap();
        assert!(load(&path).is_err());
        let _ = std::fs::remove_dir_all(&root);
    }
}
