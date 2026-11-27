#[path = "../tick_bars.rs"]
mod tick_bars;
use std::path::Path;
fn main() {
    let args = std::env::args().collect::<Vec<_>>();
    let run = || -> Result<(), String> {
        if !(4..=5).contains(&args.len()) {
            return Err(
                "用法：tick_bars Tick檔案 新輸出目錄 週期秒數 [sessions|對齊位移秒數]".into(),
            );
        }
        let seconds = args[3].parse().map_err(|_| "週期秒數不是整數")?;
        let sessions = args.len() == 5 && args[4] == "sessions";
        let anchor = if args.len() == 5 && !sessions {
            args[4].parse().map_err(|_| "對齊位移不是整數")?
        } else {
            0
        };
        let result = if sessions {
            tick_bars::convert_sessions(Path::new(&args[1]), Path::new(&args[2]), seconds)?
        } else {
            tick_bars::convert(Path::new(&args[1]), Path::new(&args[2]), seconds, anchor)?
        };
        println!(
            "{}",
            serde_json::to_string_pretty(&result).map_err(|e| e.to_string())?
        );
        Ok(())
    };
    if let Err(e) = run() {
        eprintln!("{e}");
        std::process::exit(1)
    }
}
