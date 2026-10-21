//! 原图文件读写：样本集、示教资料、录制原图共用。读取前拒绝符号链接与 reparse point。

use std::path::Path;

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

/// 读成 8 位灰度。彩色 / 16 位图按亮度换算。
pub fn load(path: &Path) -> Result<FrameImage, String> {
    checked_source_path(path)?;
    let bytes = std::fs::read(path).map_err(|e| format!("读取 {} 失败：{e}", path.display()))?;
    decode(&bytes).map_err(|e| format!("解码 {} 失败：{e}", path.display()))
}

/// 按内容识别格式解码成 8 位灰度。工作台裁定位模板与发布包核对模板都经过这里，同样的字节得到同样的像素。
pub fn decode(bytes: &[u8]) -> Result<FrameImage, String> {
    let img = image::load_from_memory(bytes).map_err(|e| e.to_string())?.into_luma8();
    let (width, height) = img.dimensions();
    Ok(FrameImage::new(width, height, img.into_raw()))
}

/// 存成 PGM（P5）。帧录制用，不压缩、写得快。
pub fn save_pgm(path: &Path, img: &FrameImage) -> Result<(), String> {
    use std::io::Write;
    let mut f = std::io::BufWriter::new(std::fs::File::create(path).map_err(|e| e.to_string())?);
    write!(f, "P5\n{} {}\n255\n", img.width, img.height).map_err(|e| e.to_string())?;
    f.write_all(&img.pixels).map_err(|e| e.to_string())?;
    f.flush().map_err(|e| e.to_string())
}

/// 冻结示教原图：按编号只写一次（定位模板身份的前提，见 `recipe::FixtureSpec`）。文件已在时逐字节比对，
/// 相同照常（同一圈重复采用）、不同报错，不覆盖。
pub fn save_pgm_new(path: &Path, img: &FrameImage) -> Result<(), String> {
    use std::io::Write;
    let mut bytes = format!("P5\n{} {}\n255\n", img.width, img.height).into_bytes();
    bytes.extend_from_slice(&img.pixels);
    match std::fs::OpenOptions::new().write(true).create_new(true).open(path) {
        Ok(mut file) => {
            if let Err(e) = file.write_all(&bytes).and_then(|_| file.sync_all()) {
                drop(file);
                let _ = std::fs::remove_file(path);
                return Err(format!("写示教原图 {} 失败：{e}", path.display()));
            }
            Ok(())
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            checked_source_path(path)?;
            let existing = std::fs::read(path).map_err(|e| format!("读取已有示教原图 {} 失败：{e}", path.display()))?;
            if existing == bytes { Ok(()) } else {
                Err(format!("示教图编号 {} 已存在且内容不同，不能覆盖（定位模板按图编号认来源）", path.file_stem().and_then(|s| s.to_str()).unwrap_or("?")))
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
        // 防的回归：冻结图按编号覆盖写（同一圈重复采用、编号碰撞）后，定位框的来源编号没变、模板像素却变了，
        // 测量定义里的模板身份 (来源, 框, 外圈) 就不再能证明模板相同
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

    #[cfg(windows)]
    #[test]
    fn junction_swapped_in_after_first_read_is_rejected_on_next_read() {
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let root = std::env::temp_dir().join(format!("gluesight-image-file-{}-{nanos}", std::process::id()));
        let (first, second, saved) = (root.join("cycle-first"), root.join("cycle-second"), root.join("saved-first"));
        for (path, pixel) in [(&first, 17), (&second, 91)] {
            std::fs::create_dir_all(path).unwrap();
            save_pgm(&path.join("image.pgm"), &FrameImage::new(1, 1, vec![pixel])).unwrap();
        }
        assert_eq!(load(&first.join("image.pgm")).unwrap().pixels, [17]);
        std::fs::rename(&first, &saved).unwrap();
        let status = std::process::Command::new("cmd").args(["/C", "mklink", "/J"]).arg(&first).arg(&second).output().unwrap();
        assert!(status.status.success(), "{}", String::from_utf8_lossy(&status.stderr));
        assert!(load(&first.join("image.pgm")).unwrap_err().contains("reparse"));
        assert_eq!(load(&second.join("image.pgm")).unwrap().pixels, [91]);
        std::fs::remove_dir(&first).unwrap();
        let _ = std::fs::remove_dir_all(&root);
    }
}
