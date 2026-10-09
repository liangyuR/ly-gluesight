// Use exactly the image dependency linked into GlueSight's replay decoder.
use std::{env, fs, path::Path};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = env::args().collect();
    if args.len() != 3 { return Err("usage: normalizer operations.json report.json".into()); }
    let operations: serde_json::Value = serde_json::from_slice(&fs::read(&args[1])?)?;
    let mut rows = Vec::new();
    for operation in operations.as_array().ok_or("operations must be an array")? {
        let source = Path::new(operation["source"].as_str().ok_or("missing source")?);
        let destination = Path::new(operation["destination"].as_str().ok_or("missing destination")?);
        if destination.exists() { return Err(format!("destination exists: {}", destination.display()).into()); }
        let gray = image::open(source)?.into_luma8();
        fs::create_dir_all(destination.parent().ok_or("missing parent")?)?;
        gray.save_with_format(destination, image::ImageFormat::Png)?;
        let decoded = image::open(destination)?.into_luma8();
        if gray.dimensions() != decoded.dimensions() || gray.as_raw() != decoded.as_raw() {
            return Err(format!("software pixel mismatch: {}", source.display()).into());
        }
        rows.push(serde_json::json!({"source":source,"destination":destination,
            "width":gray.width(),"height":gray.height(),"mode":"L8","softwarePixelsEqual":true}));
    }
    fs::write(&args[2], serde_json::to_vec_pretty(&serde_json::json!({
        "decoder":"image 0.25.10; image::open(...).into_luma8(); same as GlueSight replay::load",
        "converted":rows.len(),"rows":rows}))?)?;
    println!("Converted and checked {} images against the software decoder", rows.len());
    Ok(())
}
