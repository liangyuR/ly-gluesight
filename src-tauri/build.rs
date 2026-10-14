fn main() {
    println!("cargo:rerun-if-changed=../packaging/runtime.json");
    let text = std::fs::read_to_string("../packaging/runtime.json").expect("读取 packaging/runtime.json 失败");
    let config: serde_json::Value = serde_json::from_str(&text).expect("packaging/runtime.json 无效");
    let series = config["lyflow"]["series"].as_str().expect("packaging/runtime.json 缺少 lyflow.series");
    println!("cargo:rustc-env=GLUESIGHT_LYFLOW_SERIES={series}");
    tauri_build::build()
}
