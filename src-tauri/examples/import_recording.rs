//! 原样导入现场的一件录制到项目的历史记录（保留 cycleId、SN、时间、帧计数与录制时的配方），供示教后回传现场：
//! 回传包导入现场时，示教件原图要按同一 cycleId 从现场的历史记录补回，帧记录须逐行相同。
//!
//! 用法：import_recording --db <项目目录>/inspection.db --records <真机录制根> --part <录制目录（含 part.json）>

use std::path::PathBuf;

const USAGE: &str =
    "用法：import_recording --db <项目目录>/inspection.db --records <真机录制根> --part <录制目录>";

fn run() -> Result<String, String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() != 6 {
        return Err("参数个数不对".into());
    }
    let mut db = None;
    let mut records = None;
    let mut part = None;
    for pair in args.chunks(2) {
        let value = Some(PathBuf::from(&pair[1]));
        match pair[0].as_str() {
            "--db" => db = value,
            "--records" => records = value,
            "--part" => part = value,
            other => return Err(format!("未知参数 {other}")),
        }
    }
    let (Some(db), Some(records), Some(part)) = (db, records, part) else {
        return Err("--db、--records、--part 都要给出".into());
    };
    let imported = gluesight_lib::probe_api::import_recording(&db, &records, &part)?;
    serde_json::to_string(&imported).map_err(|e| e.to_string())
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
