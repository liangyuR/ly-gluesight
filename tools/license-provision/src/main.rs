use argon2::{password_hash::SaltString, Argon2, PasswordHasher};
use std::{fs::OpenOptions, io::Write, path::Path};

mod config;

fn write_new(path: &Path, bytes: &[u8]) -> Result<(), Box<dyn std::error::Error>> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.len() != 2 {
        return Err(
            "用法：gluesight-license-provision <应用公钥配置.json> <受保护输出目录> 或 --check-release <配置.json>".into(),
        );
    }
    let check_only = args[0] == "--check-release";
    let text = std::fs::read_to_string(&args[usize::from(check_only)])?;
    let mut config: serde_json::Value = serde_json::from_str(text.trim_start_matches('\u{feff}'))?;
    config::validate(&config, false)?;
    if check_only {
        println!("正式授权配置检查通过");
        return Ok(());
    }
    let output = Path::new(&args[1]);
    if !output.is_dir() {
        return Err(
            "请先创建仅管理员可访问的输出目录（Windows 使用 New-LicenseProvisioning.ps1）".into(),
        );
    }
    let config_path = output.join("license.local.json");
    let password_path = output.join("emergency-password.txt");
    if config_path.exists() || password_path.exists() {
        return Err("拒绝覆盖已有配置或应急密码；复用原配置即可保持固定密码".into());
    }
    let mut random = [0u8; 40];
    getrandom::getrandom(&mut random).map_err(|_| "无法生成安全随机数")?;
    let password: String = random[..24].iter().map(|b| format!("{b:02x}")).collect();
    let salt = SaltString::encode_b64(&random[24..]).map_err(|_| "生成盐失败")?;
    config["emergencyPasswordHash"] = Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map_err(|_| "生成密码校验值失败")?
        .to_string()
        .into();
    write_new(&password_path, password.as_bytes())?;
    write_new(&config_path, &serde_json::to_vec_pretty(&config)?)?;
    println!("已生成内置配置与固定应急密码；密码仅保存在输出目录的 emergency-password.txt，不会打印。每次应急放行 72 小时。不要提交或分发密码文件给客户。");
    Ok(())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
