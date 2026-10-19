//! 配置文件读写：写临时文件再改名，写到一半断电也不会留下半个文件；读不了时先把原文件备份，不悄悄覆盖。

use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use chrono::Local;
use serde::de::DeserializeOwned;

pub fn write_atomic(path: &Path, text: &str) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("创建目录 {} 失败：{e}", dir.display()))?;
    }
    let tmp = path.with_extension("tmp");
    let fail = |e: std::io::Error| format!("写 {} 失败：{e}", path.display());
    std::fs::write(&tmp, text).map_err(fail)?;
    std::fs::rename(&tmp, path).map_err(fail)
}

/// 读文本文件。记事本、PowerShell 5 存的 UTF-8 开头带 BOM，去掉。
pub fn read_text(path: &Path) -> std::io::Result<String> {
    let text = String::from_utf8(std::fs::read(path)?).map_err(|e| std::io::Error::new(ErrorKind::InvalidData, e))?;
    Ok(text.strip_prefix('\u{feff}').map(str::to_string).unwrap_or(text))
}

/// 读 JSON 配置。文件不存在返回 Ok(None)；读不了（编码不对、被占用）或解析不了时把原文件备份成
/// `<名>.broken-<时间>.json`，返回给人看的原因，调用方用默认值继续并记日志。
pub fn read_json<T: DeserializeOwned>(path: &Path) -> Result<Option<T>, String> {
    let why = match read_text(path) {
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(None),
        Err(e) => e.to_string(),
        Ok(text) => match serde_json::from_str(&text) {
            Ok(v) => return Ok(Some(v)),
            Err(e) => e.to_string(),
        },
    };
    let kept = match backup(path, "broken") {
        Ok(backup) => format!("原文件已备份为 {}", backup.display()),
        Err(e) => format!("备份也失败了（{e}）"),
    };
    Err(format!("{} 读不了（{why}），{kept}，先按默认值运行", path.display()))
}

/// 把配置文件复制成 `<名>.<kind>-<时间>.json`，返回备份路径。
pub fn backup(path: &Path, kind: &str) -> std::io::Result<PathBuf> {
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("config");
    let backup = path.with_file_name(format!("{stem}.{kind}-{}.json", Local::now().format("%Y%m%d-%H%M%S")));
    std::fs::copy(path, &backup)?;
    Ok(backup)
}
