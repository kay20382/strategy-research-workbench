//! 桌面工作台與 MCP 共用的策略檢查及候選版回測。
use crate::{strategy_library as lib, workbench as w};
use serde_json::{json, Value};
use std::{collections::BTreeMap, fs, io::BufReader, path::Path};

pub fn validate_closed_trades(trades: &Value) -> Value {
    let Some(trades) = trades.as_array() else {
        return json!({"status":"unverified","reason":"缺少可驗收的逐筆交易"});
    };
    let mut violations = Vec::new();
    let mut invalid = 0;
    for (index, trade) in trades.iter().enumerate() {
        let reason = match (trade["entry_bar"].as_u64(), trade["exit_bar"].as_u64()) {
            (Some(entry), Some(exit)) if exit > entry => continue,
            (Some(entry), Some(exit)) if exit == entry => "same_bar",
            (Some(_), Some(_)) => "reversed_bars",
            _ => { invalid += 1; "invalid_bar" }
        };
        violations.push(json!({"trade_id":trade["id"].as_u64().unwrap_or(index as u64 + 1),
            "entry_bar":trade["entry_bar"],"exit_bar":trade["exit_bar"],"entry_label":trade["entry_label"],
            "exit_label":trade["exit_label"],"entry_datetime":trade["entry_datetime"],
            "exit_datetime":trade["exit_datetime"],"reason":reason}));
    }
    let status = if invalid > 0 {"unverified"} else if violations.is_empty() {"passed"} else {"rejected"};
    json!({"status":status,"rule":"exit_bar > entry_bar","checked_trades":trades.len(),
        "violation_count":violations.len(),"violations":violations})
}

pub fn trade_validation(receipt: &Value) -> Result<Value, String> {
    if receipt["trade_validation"].is_object() {
        return Ok(receipt["trade_validation"].clone());
    }
    #[derive(serde::Deserialize)]
    struct Report { #[serde(default)] trades: Value }
    let path = receipt["artifacts"]["json"].as_str().ok_or("缺少逐筆交易報表")?;
    let file = fs::File::open(path).map_err(|e| e.to_string())?;
    let report: Report = serde_json::from_reader(BufReader::new(file)).map_err(|e| e.to_string())?;
    Ok(validate_closed_trades(&report.trades))
}

pub fn require_trade_validation(receipt: &Value) -> Result<(), String> {
    let validation = trade_validation(receipt).map_err(|error| format!("無法驗收逐筆成交：{error}"))?;
    if validation["status"] == "passed" {
        return Ok(());
    }
    let id = receipt["run_id"].as_str().unwrap_or("");
    let count = validation["violation_count"].as_u64().unwrap_or(0);
    let mut message = format!("逐筆成交驗收未通過（{id}）：{count} 筆不符合 exit_bar > entry_bar");
    if let Some(first) = validation["violations"].as_array().and_then(|rows| rows.first()) {
        message.push_str(&format!("。首筆 #{}，進場棒 {}（{}），出場棒 {}（{}）", first["trade_id"], first["entry_bar"],
            first["entry_label"].as_str().unwrap_or(""), first["exit_bar"], first["exit_label"].as_str().unwrap_or("")));
    }
    if validation["status"] == "unverified" { message.push_str("。成交棒資料不完整"); }
    message.push_str("。完整明細可依回測編號查閱原始報表。");
    Err(message)
}

pub fn check(root: &Path, doc: &lib::Document) -> Result<Value, String> {
    compile_document(root,doc)?;
    Ok(json!({"message":"相容引擎語法檢查通過；非原廠MC編譯"}))
}

pub(crate) fn compile_document(root: &Path, doc: &lib::Document) -> Result<crate::powerlanguage::Program, String> {
    let mut functions = BTreeMap::new();
    if let Some(files) = doc.task["function_files"].as_object() {
        for (name, path) in files {
            functions.insert(
                name.clone(),
                fs::read_to_string(w::absolute(root, path.as_str().ok_or("函式路徑無效")?))
                    .map_err(|e| e.to_string())?,
            );
        }
    }
    if let Some(inline) = doc.task["parameters"]["functions"].as_object() {
        if !functions.is_empty() && !inline.is_empty() {
            return Err("functions與function_files不可同時提供".into());
        }
        for (name, source) in inline {
            functions.insert(
                name.clone(),
                source.as_str().ok_or("函式內容無效")?.to_owned(),
            );
        }
    }
    if doc.kind == "function" {
        functions.insert(doc.name.clone(), doc.source.clone());
    }
    let inputs = serde_json::from_value(
        doc.task["parameters"]["inputs"]
            .as_object()
            .map(|x| json!(x))
            .unwrap_or(json!({})),
    )
    .map_err(|e| e.to_string())?;
    crate::powerlanguage::Program::with_functions(
        if doc.kind == "function" {
            ""
        } else {
            &doc.source
        },
        &inputs,
        &functions,
    )
}

fn comparison_inputs(receipt: &Value) -> Result<Value, String> {
    let artifact = |key: &str| -> Result<&Path, String> {
        receipt["artifacts"][key]
            .as_str()
            .map(Path::new)
            .ok_or_else(|| format!("缺少回測產物：{key}"))
    };
    let report = w::read_json(artifact("json")?)?;
    let bars = report["provenance"]["bars_sha256"]
        .as_str()
        .ok_or("缺少回測行情雜湊")?;
    let functions = w::read_json(artifact("functions")?)?;
    let functions: Vec<Value> = functions
        .as_array()
        .ok_or("函式來源清單無效")?
        .iter()
        .map(|f| json!([f["name"], f["sha256"]]))
        .collect();
    Ok(json!({
        "bars": bars,
        "config": receipt["configuration_sha256"],
        "engine": receipt["engine_sha256"],
        "functions": functions,
        "tick_execution": report["tick_execution"],
    }))
}

pub fn optimization_run(
    root: &Path,
    doc: &lib::Document,
    baseline: Option<&Value>,
    runtime: &str,
    log: impl FnMut(String),
) -> Result<Value, String> {
    optimization_run_controlled(root, doc, baseline, runtime, log, None)
}

pub fn optimization_run_controlled(
    root: &Path,
    doc: &lib::Document,
    baseline: Option<&Value>,
    runtime: &str,
    log: impl FnMut(String),
    stop: Option<&std::sync::atomic::AtomicBool>,
) -> Result<Value, String> {
    if doc.kind != "strategy" {
        return Err("請選擇可回測的 PL 策略。".into());
    }
    check(root, doc)?;
    let mut receipt = if let Some(stop) = stop {
        w::run_controlled(root, doc.task.clone(), &doc.source, log, stop)?
    } else {
        w::run(root, doc.task.clone(), &doc.source, false, None, log)?
    };
    if receipt["status"] != "completed" {
        return Err(format!(
            "回測失敗：{}（{}）",
            receipt["error"].as_str().unwrap_or("請查看執行紀錄"),
            receipt["run_id"]
        ));
    }
    if stop.is_some_and(|flag| flag.load(std::sync::atomic::Ordering::Acquire)) {
        return Err("工作已取消".into());
    }
    require_trade_validation(&receipt)?;
    let inputs = comparison_inputs(&receipt)?;
    if let Some(baseline) = baseline {
        if baseline != &inputs {
            return Err("行情、函式或回測設定與基準不同，無法比較。請重新執行優化。".into());
        }
    }
    receipt["ui_runtime"] = json!(runtime);
    receipt["optimization_inputs"] = inputs;
    w::write_json(
        Path::new(receipt["result_path"].as_str().ok_or("缺少收據路徑")?),
        &receipt,
    )?;
    Ok(receipt)
}

pub fn view(receipt: &Value, page: usize) -> Result<Value, String> {
    if receipt["status"] != "completed" {
        return Ok(json!({"receipt":receipt,"equity":[],"trades":[],"trade_count":0}));
    }
    let r = w::read_json(Path::new(
        receipt["artifacts"]["json"]
            .as_str()
            .ok_or("缺少報表路徑")?,
    ))?;
    let trades = r["trades"].as_array().ok_or("缺少逐筆交易")?;
    let mut receipt = receipt.clone();
    if !receipt["trade_validation"].is_object() {
        receipt["trade_validation"] = validate_closed_trades(&r["trades"]);
    }
    let equity = r["equity"].as_array().ok_or("缺少權益資料")?;
    let step = equity.len().div_ceil(1800).max(1);
    let mut points: Vec<Value> = equity.iter().step_by(step).cloned().collect();
    if let Some(last) = equity.last() {
        if points.last() != Some(last) {
            points.push(last.clone());
        }
    }
    Ok(
        json!({"receipt":receipt,"equity":points,"equity_total":equity.len(),"trades":trades.iter().skip(page*100).take(100).collect::<Vec<_>>(),"trade_count":trades.len(),"page":page,"bar_count":r["bar_count"],"open_trade":r["open_trade"]}),
    )
}
