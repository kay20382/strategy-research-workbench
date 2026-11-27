use recharts_core::workbench as w;
use serde_json::json;
use std::{fs, path::PathBuf};
fn main() {
    let result = (|| -> Result<serde_json::Value, String> {
        let root = w::root();
        let args: Vec<_> = std::env::args().skip(1).collect();
        let mut task = None;
        let mut pl = None;
        let mut destination = None;
        let mut excel = true;
        let mut i = 0;
        while i < args.len() {
            match args[i].as_str(){"--task"|"--pl"|"--report-directory"=>{let key=&args[i];i+=1;let value=args.get(i).ok_or("選項缺少值")?;match key.as_str(){"--task"=>task=Some(PathBuf::from(value)),"--pl"=>pl=Some(PathBuf::from(value)),_=>destination=Some(PathBuf::from(value))}},"--no-excel"=>excel=false,_=>return Err("用法：pl_workbench_cli --task 任務.json [--pl 程式.pl] [--no-excel] [--report-directory 目錄]".into())}
            i += 1;
        }
        let t = w::load_task(&root, &task.ok_or("請提供 --task")?)?;
        let source = fs::read_to_string(
            pl.unwrap_or_else(|| PathBuf::from(t["strategy"].as_str().unwrap_or(""))),
        )
        .map_err(|e| e.to_string())?;
        w::run(
            &root,
            t,
            source.trim_start_matches('\u{feff}'),
            excel,
            destination.as_deref(),
            |s| eprintln!("{s}"),
        )
    })();
    let receipt = result.unwrap_or_else(|e| json!({"status":"failed","error":e}));
    println!("{}", receipt);
    if receipt["status"] != "completed" {
        std::process::exit(1);
    }
}
