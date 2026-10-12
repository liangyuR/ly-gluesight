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
    let img = image::open(path).map_err(|e| format!("解码 {} 失败：{e}", path.display()))?.into_luma8();
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

#[cfg(all(test, windows))]
mod tests {
    use super::*;

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
