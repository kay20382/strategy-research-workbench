//! PL 工作台的共用 Rust 流程；原生 UI 與 CLI 共用。
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
#[cfg(windows)]
use std::os::windows::process::CommandExt;
use std::{
    collections::HashSet,
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::atomic::{AtomicBool, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

pub const LIMITATION: &str =
    "自製 MC 相容引擎，僅支援已實作的 PL 語法與函式；尚未通過全部 MC 完整報告等價驗證。";
const STOPPED: &str = "已關閉策略執行開關，回測停止。";
pub fn root() -> PathBuf {
    if let Some(path) = std::env::var_os("REA_WORKBENCH_ROOT").map(PathBuf::from) {
        if path.is_absolute() && path.join("godot-workbench/project.godot").is_file() {
            return path;
        }
    }
    if let Ok(executable) = std::env::current_exe() {
        if let Some(path) = executable
            .ancestors()
            .find(|path| path.join("godot-workbench/project.godot").is_file())
        {
            return path.to_path_buf();
        }
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf()
}
pub fn absolute(root: &Path, path: &str) -> PathBuf {
    let p = Path::new(path);
    if p.is_absolute() {
        p.into()
    } else {
        root.join(p)
    }
}
pub fn read_json(path: &Path) -> Result<Value, String> {
    serde_json::from_str(
        fs::read_to_string(path)
            .map_err(|e| format!("{}：{e}", path.display()))?
            .trim_start_matches('\u{feff}'),
    )
    .map_err(|e| e.to_string())
}
pub fn write_json(path: &Path, value: &Value) -> Result<(), String> {
    fs::write(
        path,
        serde_json::to_vec_pretty(value).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())
}
fn hash(raw: &[u8]) -> String {
    format!("{:x}", Sha256::digest(raw))
}
pub fn load_task(root: &Path, path: &Path) -> Result<Value, String> {
    let mut t = read_json(path)?;
    for key in ["database", "strategy", "instrument_specifications"] {
        if let Some(p) = t[key].as_str() {
            t[key] = json!(absolute(root, p));
        }
    }
    if let Some(files) = t["function_files"].as_object_mut() {
        for p in files.values_mut() {
            *p = json!(absolute(root, p.as_str().ok_or("函式路徑須為文字")?));
        }
    }
    Ok(t)
}
pub fn datasets(root: &Path, database: &str) -> Result<Value, String> {
    let c = rusqlite::Connection::open_with_flags(
        absolute(root, database),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .map_err(|e| e.to_string())?;
    let mut query=c.prepare("SELECT id,symbol,kind,row_count,first_datetime,last_datetime FROM datasets ORDER BY id").map_err(|e|e.to_string())?;
    let rows=query.query_map([],|r|Ok(json!({"id":r.get::<_,i64>(0)?,"symbol":r.get::<_,String>(1)?,"kind":r.get::<_,String>(2)?,"row_count":r.get::<_,i64>(3)?,"first_datetime":r.get::<_,Option<String>>(4)?,"last_datetime":r.get::<_,Option<String>>(5)?}))).map_err(|e|e.to_string())?;
    Ok(Value::Array(
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?,
    ))
}
pub fn valid_filename(s: &str) -> bool {
    !s.trim().is_empty()
        && !s.ends_with(['.', ' '])
        && !s.chars().any(|c| c < ' ' || "<>:\"/\\|?*".contains(c))
        && ![
            "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7",
            "COM8", "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
        ]
        .contains(
            &s.split('.')
                .next()
                .unwrap_or("")
                .to_ascii_uppercase()
                .as_str(),
        )
}
pub fn tool(home: &Path, relative: &str, fallback: &str) -> PathBuf {
    let p = home
        .join(".cache/codex-runtimes/codex-primary-runtime/dependencies")
        .join(relative);
    if p.exists() {
        p
    } else {
        PathBuf::from(fallback)
    }
}
pub fn execute(
    root: &Path,
    program: &Path,
    args: &[PathBuf],
    logfile: &Path,
    log: &mut dyn FnMut(String),
) -> Result<(), String> {
    execute_with_stop(root, program, args, logfile, log, None)
}

pub fn execute_with_stop(
    root: &Path,
    program: &Path,
    args: &[PathBuf],
    logfile: &Path,
    log: &mut dyn FnMut(String),
    stop: Option<&AtomicBool>,
) -> Result<(), String> {
    if stop.is_some_and(|flag| flag.load(Ordering::Acquire)) {
        return Err(STOPPED.into());
    }
    log(format!(
        "執行：{}",
        program.file_name().unwrap_or_default().to_string_lossy()
    ));
    let output = fs::File::create(logfile).map_err(|e| e.to_string())?;
    let mut cmd = Command::new(program);
    cmd.args(args)
        .current_dir(root)
        .env("REA_EXPORT_PREVIEW", "0")
        .env("PYTHONIOENCODING", "utf-8")
        .stdin(Stdio::null())
        .stdout(Stdio::from(output.try_clone().map_err(|e| e.to_string())?))
        .stderr(Stdio::from(output));
    #[cfg(windows)]
    cmd.creation_flags(0x08000000);
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("無法啟動 {}：{e}", program.display()))?;
    let stop_child = |child: &mut std::process::Child| {
        // 報表匯出會啟動繪圖程序，Windows 下需一起停止。
        #[cfg(windows)]
        if let Some(system_root) = std::env::var_os("SystemRoot") {
            let taskkill = PathBuf::from(system_root).join("System32/taskkill.exe");
            if Command::new(taskkill)
                .args(["/F", "/T", "/PID", &child.id().to_string()])
                .creation_flags(0x08000000)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .is_ok_and(|status| status.success())
            {
                return Ok(());
            }
        }
        child.kill()
    };
    let start = std::time::Instant::now();
    let mut last_progress = 0;
    let status = loop {
        if let Some(s) = child.try_wait().map_err(|e| e.to_string())? {
            break s;
        }
        if let Some(flag) = stop.filter(|flag| flag.load(Ordering::Acquire)) {
            match stop_child(&mut child) {
                Ok(()) => {
                    child.wait().map_err(|e| e.to_string())?;
                    return Err(STOPPED.into());
                }
                Err(error) => match child.try_wait().map_err(|e| e.to_string())? {
                    Some(status) => break status,
                    None => {
                        log(format!("未能停止子程序，將繼續等待程序結束：{error}"));
                        flag.store(false, Ordering::Release);
                    }
                },
            }
        }
        let elapsed = start.elapsed().as_secs();
        if elapsed > 1800 {
            let _ = stop_child(&mut child);
            let _ = child.wait();
            return Err(format!("執行逾時，紀錄：{}", logfile.display()));
        }
        if stop.is_some() && elapsed >= last_progress + 2 {
            log(format!("子程序執行中，已過 {elapsed} 秒"));
            last_progress = elapsed;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    };
    let text = fs::read_to_string(logfile).unwrap_or_default();
    log(text
        .chars()
        .rev()
        .take(5000)
        .collect::<String>()
        .chars()
        .rev()
        .collect());
    if status.success() {
        Ok(())
    } else {
        Err(format!(
            "執行失敗（{status}），紀錄：{}\n{}",
            logfile.display(),
            text.chars()
                .rev()
                .take(1500)
                .collect::<String>()
                .chars()
                .rev()
                .collect::<String>()
        ))
    }
}

/// 成功或流程失敗均回傳完整收據；僅輸入／建立收據前失敗使用 Err。
pub fn prepare_market_task(root: &Path, task: &mut Value) -> Result<(), String> {
    if task["market"].is_null() {
        return Ok(());
    }
    let database = absolute(root, task["database"].as_str().ok_or("缺少資料庫路徑")?);
    crate::canonical_market::initialize(root, &database)?;
    let symbol = task["market"]["symbol"]
        .as_str()
        .ok_or("請選擇商品")?
        .trim()
        .to_uppercase();
    let unit = task["market"]["unit"]
        .as_str()
        .ok_or("請選擇秒K、分K或日K")?
        .to_owned();
    let period = task["market"]["period"]
        .as_u64()
        .filter(|n| *n > 0)
        .ok_or("K棒週期須為正整數")?;
    let kind = match unit.as_str() {
        "seconds" if period <= 86400 => "tick",
        "minutes" if period <= 1440 => "minute",
        "days" if period <= 365 => "daily",
        _ => return Err("週期超出支援範圍：秒1～86400、分1～1440、日1～365".into()),
    };
    let id = crate::canonical_market::resolve(&database, &symbol, kind)?;
    task["market"]["symbol"] = json!(symbol);
    task["parameters"]["dataset_id"] = json!(id);
    task["parameters"]["interval_seconds"] = Value::Null;
    task["parameters"]["interval_days"] = Value::Null;
    task["parameters"]["source_interval_minutes"] = Value::Null;
    task["parameters"]["interval_minutes"] = json!(if unit == "minutes" { period } else { 0 });
    if unit == "seconds" {
        task["parameters"]["interval_seconds"] = json!(period);
        task["parameters"]["timestamp_role"] = json!("end");
    }
    if unit == "minutes" {
        task["parameters"]["source_interval_minutes"] = json!(1);
    }
    if unit == "days" {
        task["parameters"]["interval_days"] = json!(period);
    }
    let txf = symbol == "TXF1" && unit != "days" && task["market"]["session_profile"] != "all_day";
    task["parameters"]["sessions"] = if txf {
        json!([{"name":"日盤","start_minute":525,"end_minute":825},{"name":"夜盤","start_minute":900,"end_minute":300}])
    } else {
        json!([])
    };
    task["market"]["resolved_kind"] = json!(kind);
    task["market"]["resolved_dataset_id"] = json!(id);
    crate::quote_manager::apply_to_task(&database, &symbol, &unit, task)?;
    Ok(())
}
pub fn run(
    root: &Path,
    task: Value,
    source: &str,
    excel: bool,
    destination: Option<&Path>,
    log: impl FnMut(String),
) -> Result<Value, String> {
    run_workbench(root, task, source, excel, destination, log, None)
}

pub fn run_controlled(
    root: &Path,
    task: Value,
    source: &str,
    log: impl FnMut(String),
    stop: &AtomicBool,
) -> Result<Value, String> {
    run_workbench(root, task, source, false, None, log, Some(stop))
}

fn run_workbench(
    root: &Path,
    mut task: Value,
    source: &str,
    excel: bool,
    destination: Option<&Path>,
    mut log: impl FnMut(String),
    stop: Option<&AtomicBool>,
) -> Result<Value, String> {
    prepare_market_task(root, &mut task)?;
    if source.trim().is_empty() {
        return Err("請輸入 PL 程式碼".into());
    }
    let name = task["parameters"]["strategy_name"]
        .as_str()
        .unwrap_or("")
        .trim()
        .to_owned();
    if !valid_filename(&name) {
        return Err("請填寫可用於 Windows 檔名的策略名稱".into());
    }
    task["parameters"]["strategy_name"] = json!(name);
    let all = datasets(root, task["database"].as_str().ok_or("缺少資料庫路徑")?)?;
    let dataset = all
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["id"] == task["parameters"]["dataset_id"])
        .ok_or("所選資料集不存在")?
        .clone();
    let symbol = dataset["symbol"].as_str().ok_or("商品名稱缺漏")?;
    if !valid_filename(symbol) {
        return Err("商品名稱無法用於報告檔名".into());
    }
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_nanos();
    let runs = root.join("outputs/pl-workbench");
    fs::create_dir_all(&runs).map_err(|e| e.to_string())?;
    let mut run_stamp = nanos;
    let (id, dir) = loop {
        let id = format!("rust-{run_stamp}-{}", std::process::id());
        let dir = runs.join(&id);
        match fs::create_dir(&dir) {
            Ok(()) => break (id, dir),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => run_stamp += 1,
            Err(error) => return Err(error.to_string()),
        }
    };
    let index = dir.join("result.json");
    let mut receipt = json!({"schema":"pl-workbench-result-v1","run_id":id,"status":"running","engine":"rea-mc-compatible-v1","ui_runtime":"rust-win32","limitation":LIMITATION,"started_unix_ns":nanos.to_string(),"source_sha256":hash(source.as_bytes()),"dataset":dataset,"artifacts":{},"summary":null,"error":null,"result_path":index});
    write_json(&index, &receipt)?;
    log(format!("本次資料夾：{}", dir.display()));
    let result = (|| -> Result<(), String> {
        let engine = root.join("rust/target/debug/backtest.exe");
        if !engine.exists() {
            return Err("找不到 Rust 回測核心，請執行 build.ps1".into());
        }
        for key in ["database", "instrument_specifications"] {
            if let Some(p) = task[key].as_str() {
                task[key] = json!(absolute(root, p));
            }
        }
        let database = PathBuf::from(task["database"].as_str().ok_or("缺少資料庫路徑")?);
        let calendar = if task["market"]["instrument_profile"].is_object() {
            task["market"]["instrument_profile"].clone()
        } else {
            crate::quote_manager::get(&database, symbol).ok().filter(|i| i.confirmed)
                .map(|i| json!(i)).unwrap_or_default()
        };
        if task["function_files"]
            .as_object()
            .is_some_and(|m| !m.is_empty())
            && task["parameters"]["functions"]
                .as_object()
                .is_some_and(|m| !m.is_empty())
        {
            return Err("functions 與 function_files 不可同時提供".into());
        }
        let mut seen = HashSet::new();
        let mut functions = json!({});
        let mut sources = vec![];
        if let Some(files) = task["function_files"].as_object() {
            for (i, (name, file)) in files.iter().enumerate() {
                if !seen.insert(name.to_ascii_lowercase()) {
                    return Err(format!("函式名稱不分大小寫重複：{name}"));
                }
                let origin = absolute(root, file.as_str().ok_or("函式路徑須為文字")?);
                let raw = fs::read(&origin).map_err(|e| e.to_string())?;
                let snapshot = dir.join(format!("function-{i:03}.pl"));
                fs::write(&snapshot, &raw).map_err(|e| e.to_string())?;
                functions[name] = json!(snapshot);
                sources.push(
                    json!({"name":name,"source":origin,"snapshot":snapshot,"sha256":hash(&raw)}),
                );
            }
        }
        task["function_files"] = functions;
        let pl = dir.join("input.pl");
        fs::write(&pl, source).map_err(|e| e.to_string())?;
        let output = dir.join("engine");
        let taskpath = dir.join("task.json");
        let sourcepath = dir.join("function-sources.json");
        task["strategy"] = json!(pl);
        task["output"] = json!(output);
        if !task["report_settings"].is_object() {
            task["report_settings"] = json!({});
        }
        task["report_settings"]["validation_note"] = json!(LIMITATION);
        task["report_settings"]["analysis_bars_path"] = json!(output.join("bars.csv"));
        if let Some(unit) = task["market"]["unit"].as_str() {
            task["report_settings"]["interval_label"] = json!(format!(
                "{} {} K",
                task["market"]["period"],
                match unit {
                    "seconds" => "秒",
                    "minutes" => "分",
                    _ => "日",
                }
            ));
        } else if let Some(minutes) = task["parameters"]["interval_minutes"].as_u64() {
            if minutes > 0 {
                task["report_settings"]["interval_label"] = json!(format!("{} 秒 K", minutes * 60));
            }
        }
        task["parameters"]["research_note"] = json!(format!(
            "{}\n{}",
            LIMITATION,
            task["parameters"]["research_note"].as_str().unwrap_or("")
        ));
        write_json(&taskpath, &task)?;
        write_json(&sourcepath, &json!(sources))?;
        receipt["artifacts"] = json!({"task":taskpath,"source":pl,"functions":sourcepath});
        #[cfg(windows)]
        match crate::pl_exchange::encode(source) {
            Ok(bytes) => {
                let file = dir.join("input-cp950.pl");
                fs::write(&file,bytes).map_err(|e| e.to_string())?;
                receipt["artifacts"]["source_cp950"] = json!(file);
            }
            Err(error) => receipt["source_cp950_error"] = json!(error),
        }
        execute_with_stop(
            root,
            &engine,
            &[taskpath.clone(), output.clone()],
            &dir.join("engine.log"),
            &mut log,
            stop,
        )?;
        let report = read_json(&output.join("report.json"))?;
        receipt["summary"] = report["summary"].clone();
        receipt["market_coverage"] = crate::quote_manager::backtest_coverage(&database, &report);
        if let Ok(stats) = crate::report_statistics::performance(&report, &calendar, Default::default()) {
            receipt["report_statistics"] = stats;
        }
        if calendar.is_object() {
            receipt["report_calendar"] = calendar;
        }
        receipt["trade_validation"] = crate::strategy_research::validate_closed_trades(&report["trades"]);
        let validation_path = dir.join("trade-validation.json");
        write_json(&validation_path, &receipt["trade_validation"])?;
        receipt["warnings"] = report["warnings"].clone();
        receipt["engine_sha256"] = report["engine_sha256"].clone();
        receipt["configuration_sha256"] = report["configuration_sha256"].clone();
        for (key, file) in [
            ("html", "report.html"),
            ("json", "report.json"),
            ("trades", "trades.csv"),
            ("equity", "equity.csv"),
            ("bars", "bars.csv"),
        ] {
            receipt["artifacts"][key] = json!(output.join(file));
        }
        let html = dir.join("report.html");
        match crate::workbench_report::write_html(&output.join("report.html"), &html, &receipt) {
            Ok(()) => {
                receipt["artifacts"]["engine_html"] = receipt["artifacts"]["html"].clone();
                receipt["artifacts"]["html"] = json!(html);
            }
            Err(error) => log(format!("無法產生含補充統計的 HTML，請查閱回測摘要：{error}")),
        }
        receipt["artifacts"]["trade_validation"] = json!(validation_path);
        if receipt["trade_validation"]["status"] != "passed" {
            log(format!("逐筆成交驗收未通過：{} 筆違規；原始成交保留供查核。", receipt["trade_validation"]["violation_count"].as_u64().unwrap_or(0)));
        }
        write_json(&index, &receipt)?;
        if excel {
            let pref = read_json(&root.join("data/report-output-settings.json"))?;
            let dest = destination
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from(pref["directory"].as_str().unwrap_or("outputs")));
            let dest = if dest.is_absolute() {
                dest
            } else {
                root.join(dest)
            };
            let filename = pref["filename_pattern"]
                .as_str()
                .ok_or("缺少報告檔名格式")?
                .replace("{symbol}", symbol)
                .replace("{strategy}", &name);
            let mut target = dest.join(&filename);
            if target.exists() {
                target = dest.join(&id).join(&filename);
            }
            let staging = root.join("outputs/report-exports/pl-workbench").join(&id);
            fs::create_dir_all(&staging).map_err(|e| e.to_string())?;
            let workbook = staging.join(&filename);
            let home = PathBuf::from(std::env::var("USERPROFILE").map_err(|e| e.to_string())?);
            let node = tool(&home, "node/bin/node.exe", "node.exe");
            let python = tool(&home, "python/python.exe", "python.exe");
            log("匯出 Excel；沿用既有報表繪圖及驗證模組…".into());
            execute(
                root,
                &node,
                &[
                    "--max-old-space-size=8192".into(),
                    root.join("scripts/export-report.mjs"),
                    output.join("report.json"),
                    workbook.clone(),
                    "-".into(),
                    pl,
                    taskpath,
                ],
                &dir.join("excel.log"),
                &mut log,
            )?;
            for script in ["validate-mc-report-layout.py", "validate-report-export.py"] {
                let mut args = vec![root.join("scripts").join(script), workbook.clone()];
                if script == "validate-mc-report-layout.py" {
                    args.push(output.join("report.json"));
                }
                execute(
                    root,
                    &python,
                    &args,
                    &dir.join(format!("{script}.log")),
                    &mut log,
                )?;
            }
            fs::create_dir_all(target.parent().unwrap()).map_err(|e| e.to_string())?;
            let mut file = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&target)
                .map_err(|e| e.to_string())?;
            let raw = fs::read(&workbook).map_err(|e| e.to_string())?;
            file.write_all(&raw).map_err(|e| e.to_string())?;
            file.sync_all().map_err(|e| e.to_string())?;
            receipt["artifacts"]["excel"] = json!(target);
            receipt["artifacts"]["excel_support"] = json!(staging);
        }
        Ok(())
    })();
    match result {
        Ok(()) => {
            receipt["status"] = json!("completed");
            log("回測完成".into());
        }
        Err(e) => {
            receipt["status"] = json!(if e == STOPPED { "stopped" } else { "failed" });
            receipt["error"] = json!(e);
            log(e);
        }
    }
    receipt["finished_unix_ns"] = json!(SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
        .to_string());
    write_json(&index, &receipt)?;
    Ok(receipt)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(windows)]
    #[test]
    fn controlled_backtest_stops_a_started_child() {
        let output = root()
            .join("outputs")
            .join(crate::strategy_library::fresh_id())
            .with_extension("log");
        let stop = AtomicBool::new(false);
        let started = std::thread::scope(|scope| {
            let result = scope.spawn(|| {
                execute_with_stop(
                    &root(),
                    Path::new("powershell.exe"),
                    &[
                        "-NoProfile".into(),
                        "-NonInteractive".into(),
                        "-Command".into(),
                        "Write-Output 'started'; Start-Sleep -Seconds 12".into(),
                    ],
                    &output,
                    &mut |_| {},
                    Some(&stop),
                )
            });
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            let started = loop {
                if fs::read_to_string(&output).is_ok_and(|line| line.contains("started")) {
                    break true;
                }
                if std::time::Instant::now() >= deadline {
                    break false;
                }
                std::thread::sleep(std::time::Duration::from_millis(25));
            };
            stop.store(true, Ordering::Release);
            assert_eq!(result.join().unwrap(), Err(STOPPED.into()));
            started
        });
        let _ = fs::remove_file(output);
        assert!(started);
    }
    #[test]
    fn filename_rejects_paths_and_devices() {
        for s in ["", "../報告", "CON", "a.", "a ", "a:b"] {
            assert!(!valid_filename(s));
        }
        assert!(valid_filename("REA_策略"));
    }
    #[test]
    fn loads_real_dataset_read_only() {
        let r = root();
        let t = load_task(&r, &r.join("tasks/pl-workbench-example.json")).unwrap();
        let rows = datasets(&r, t["database"].as_str().unwrap()).unwrap();
        assert!(rows.as_array().unwrap().iter().any(|v| v["id"] == 11));
    }
}
