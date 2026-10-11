#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.first().is_some_and(|arg| arg == "--check-packaged-runtime") {
        if args.len() != 2 {
            std::process::exit(2);
        }
        if let Err(error) = gluesight_lib::check_packaged_runtime(std::path::Path::new(&args[1])) {
            eprintln!("{error}");
            std::process::exit(1);
        }
        return;
    }
    gluesight_lib::run()
}
