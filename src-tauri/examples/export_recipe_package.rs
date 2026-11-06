use std::path::PathBuf;

const USAGE: &str =
    "用法：export_recipe_package --project <项目目录> --recipe <配方编号> --out <回传包.zip>";

fn run() -> Result<String, String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        return Ok(USAGE.into());
    }
    if args.len() != 6 {
        return Err("--project、--recipe、--out 都要给出，且各出现一次".into());
    }
    let mut project = None;
    let mut recipe = None;
    let mut out = None;
    for pair in args.chunks_exact(2) {
        let slot = match pair[0].as_str() {
            "--project" => &mut project,
            "--recipe" => &mut recipe,
            "--out" => &mut out,
            other => return Err(format!("未知参数 {other}")),
        };
        if slot.replace(pair[1].clone()).is_some() {
            return Err(format!("重复参数 {}", pair[0]));
        }
    }
    let (Some(project), Some(recipe), Some(out)) = (project, recipe, out) else {
        return Err("--project、--recipe、--out 都要给出".into());
    };
    let exported = gluesight_lib::probe_api::export_recipe_package(
        &PathBuf::from(project),
        &recipe,
        &PathBuf::from(out),
    )?;
    serde_json::to_string(&exported).map_err(|e| e.to_string())
}

fn main() {
    match run() {
        Ok(summary) => println!("{summary}"),
        Err(error) => {
            eprintln!("{error}\n{USAGE}");
            std::process::exit(1);
        }
    }
}
