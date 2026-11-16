#[path = "../tools/license-provision/src/config.rs"]
mod license_configuration;

fn main() {
    license_config();
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
    build_identity();
    tauri_build::build()
}

fn license_config() {
    println!("cargo:rerun-if-env-changed=GLUESIGHT_LICENSE_CONFIG");
    println!("cargo:rerun-if-changed=../tools/license-provision/src/config.rs");
    let configured = std::env::var_os("GLUESIGHT_LICENSE_CONFIG");
    let text = match configured {
        Some(path) => {
            let path = std::path::PathBuf::from(path);
            println!("cargo:rerun-if-changed={}", path.display());
            std::fs::read_to_string(path).expect("无法读取授权构建配置")
        }
        None => {
            assert!(
                std::env::var("PROFILE").as_deref() != Ok("release"),
                "正式构建必须设置 GLUESIGHT_LICENSE_CONFIG，禁止发布未配置授权的安装包"
            );
            r#"{"serverUrl":"","productId":"","trustedKeys":{},"emergencyPasswordHash":""}"#.into()
        }
    };
    let value: serde_json::Value = serde_json::from_str(text.trim_start_matches('\u{feff}'))
        .expect("授权构建配置不是有效 JSON");
    license_configuration::validate(&value, std::env::var("PROFILE").as_deref() != Ok("release"))
        .unwrap_or_else(|message| panic!("授权构建配置无效：{message}"));
    let output = std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap());
    std::fs::write(
        output.join("license-config.json"),
        serde_json::to_vec(&value).unwrap(),
    )
    .expect("无法生成内置授权配置");
}

fn git(args: &[&str]) -> Option<String> {
    let out = std::process::Command::new("git")
        .args(args)
        .output()
        .ok()
        .filter(|o| o.status.success())?;
    Some(String::from_utf8(out.stdout).ok()?.trim().to_string()).filter(|s| !s.is_empty())
}

/// 运行日志与关于页用的构建身份：CI 可用 GLUESIGHT_GIT_SHA 覆盖；取不到时为 unknown，工作区有未提交改动时带 -dirty。
fn build_identity() {
    println!("cargo:rerun-if-env-changed=GLUESIGHT_GIT_SHA");
    println!("cargo:rerun-if-changed=src");
    let branch = git(&["symbolic-ref", "-q", "HEAD"]);
    for path in std::iter::once(git(&["rev-parse", "--git-path", "HEAD"]))
        .chain([branch.and_then(|r| git(&["rev-parse", "--git-path", &r]))])
        .flatten()
    {
        if std::path::Path::new(&path).exists() {
            println!("cargo:rerun-if-changed={path}");
        }
    }
    let sha = std::env::var("GLUESIGHT_GIT_SHA")
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(|| {
            let sha = git(&["rev-parse", "--short=12", "HEAD"])?;
            let dirty = git(&[
                "--no-optional-locks",
                "status",
                "--porcelain",
                "--untracked-files=no",
            ])
            .is_some();
            Some(if dirty { format!("{sha}-dirty") } else { sha })
        });
    println!(
        "cargo:rustc-env=GLUESIGHT_GIT_SHA={}",
        sha.unwrap_or_else(|| "unknown".into())
    );
    let built = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis());
    println!("cargo:rustc-env=GLUESIGHT_BUILD_MS={built}");
}
