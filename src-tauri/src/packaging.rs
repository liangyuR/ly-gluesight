use std::path::Path;

use serde_json::{json, Value};

use crate::vision::Engine;

pub fn check_packaged_runtime(report_path: &Path) -> Result<(), String> {
    let result = check_runtime();
    let report = match &result {
        Ok(report) => report.clone(),
        Err(error) => json!({"status": "failed", "error": error}),
    };
    let bytes = serde_json::to_vec_pretty(&report).map_err(|error| error.to_string())?;
    std::fs::write(report_path, bytes).map_err(|error| format!("写入运行库验证报告失败：{error}"))?;
    result.map(|_| ())
}

fn check_runtime() -> Result<Value, String> {
    let executable = std::env::current_exe().map_err(|error| error.to_string())?;
    let root = executable.parent().ok_or("无法解析安装目录")?;
    let engine = Engine::load(&root.join("runtime/lyflow/lyflow_core.dll"))?;
    let api = crate::mvs::load_bundled_api()?;
    let devices = crate::mvs::enumerate_with_api(&api)?;
    Ok(json!({
        "status": "ok", "executable": executable,
        "lyflow": {"path": engine.path, "version": engine.version,
            "clientAbiVersion": lyflow_client::ABI_VERSION},
        "mvs": {"path": api.library_path, "version": api.version, "deviceCount": devices.len()},
        "acceptance": "synthetic pixels and SDK enumeration; not physical camera or field acceptance"
    }))
}
