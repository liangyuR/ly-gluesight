fn main() {
    println!("cargo:rerun-if-changed=../packaging/runtime.json");
    let text = std::fs::read_to_string("../packaging/runtime.json").expect("读取 packaging/runtime.json 失败");
    let config: serde_json::Value = serde_json::from_str(&text).expect("packaging/runtime.json 无效");
    let series = config["lyflow"]["series"].as_str().expect("packaging/runtime.json 缺少 lyflow.series");
    println!("cargo:rustc-env=GLUESIGHT_LYFLOW_SERIES={series}");
    // 必需算子以 runtime.json 为唯一来源：打包脚本按它生成运行库清单，程序加载核心库时按它核对
    let operators: Vec<&str> = config["requiredOperators"].as_array().expect("packaging/runtime.json 缺少 requiredOperators")
        .iter().map(|op| op.as_str().filter(|s| !s.is_empty() && !s.contains(',')).expect("requiredOperators 必须是不含逗号的非空字符串"))
        .collect();
    assert!(!operators.is_empty(), "packaging/runtime.json 的 requiredOperators 不能为空");
    println!("cargo:rustc-env=GLUESIGHT_REQUIRED_OPERATORS={}", operators.join(","));
    tauri_build::build()
}
