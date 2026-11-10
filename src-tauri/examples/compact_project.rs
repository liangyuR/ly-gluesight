use std::path::{Path, PathBuf};
use std::sync::Arc;
use gluesight_lib::probe_api::{compact_project_snapshot, load_release, Engine, Prepared};

fn files(root: &Path) -> Result<Vec<PathBuf>, Box<dyn std::error::Error>> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(root)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() { out.extend(files(&entry.path())?); }
        else if entry.file_type()?.is_file() { out.push(entry.path()); }
    }
    Ok(out)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    let verify_only = args.len() == 4 && args[3] == "--verify-only";
    if args.len() != 3 && !verify_only { return Err("usage: compact_project <source-project> <new-copy> <lyflow-dll> [--verify-only]".into()); }
    let source = Path::new(&args[0]);
    let target = Path::new(&args[1]);
    let before: u64 = files(source)?.iter().map(|p| std::fs::metadata(p).unwrap().len()).sum();
    if !verify_only { compact_project_snapshot(source, target)?; }
    let retained = files(target)?;
    let after: u64 = retained.iter().map(|p| std::fs::metadata(p).unwrap().len()).sum();
    eprintln!("snapshot complete: {before} -> {after} bytes; validating images and warmup");
    let mut verified = 0;
    for path in &retained {
        if path.extension().is_some_and(|e| e == "pgm") {
            let original = source.join(path.strip_prefix(target)?);
            if original.is_file() {
                if std::fs::read(&original)? != std::fs::read(path)? {
                    return Err(format!("image changed: {}", path.display()).into());
                }
                verified += 1;
            }
        }
    }
    let engine = Arc::new(Engine::load(Path::new(&args[2]))?);
    let mut bundles = Vec::new();
    for path in retained.iter().filter(|p| p.file_name().is_some_and(|n| n == "manifest.json") && p.starts_with(target.join("vision/releases"))) {
        let value: serde_json::Value = serde_json::from_slice(&std::fs::read(path)?)?;
        let recipe = value["recipeId"].as_str().ok_or("recipeId missing")?;
        let id = value["bundleId"].as_str().ok_or("bundleId missing")?;
        let mut results = Vec::new();
        for root in [source, target] {
            let bundle = load_release(&root.join("vision/releases"), recipe, id)?;
            let expected = bundle.recipe.build()?;
            let mut prepared = Prepared::load(bundle, engine.clone(), &expected)?;
            let result = gluesight_lib::probe_api::warm(&mut prepared);
            results.push(result);
        }
        if results[0] != results[1] { return Err(format!("warmup changed: {recipe}/{id}: {results:?}").into()); }
        bundles.push(serde_json::json!({"recipe": recipe, "bundle": id, "warmup": results[1]}));
    }
    println!("{}", serde_json::to_string_pretty(&serde_json::json!({
        "sourceBytes": before, "copyBytes": after, "savedBytes": before-after,
        "verifiedImages": verified, "bundles": bundles
    }))?);
    Ok(())
}
