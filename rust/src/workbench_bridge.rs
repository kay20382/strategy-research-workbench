//! Godot 工作台僅處理介面；共用原有 Rust 執行與策略保存流程。
use godot::prelude::*;
use serde_json::{json, Value};
use std::{fs, io::BufReader, path::Path, sync::mpsc};
use std::sync::{Arc, atomic::{AtomicBool, Ordering}};
use crate::{strategy_library as lib, strategy_research::{check, optimization_run, view}, workbench as w};

#[derive(GodotClass)]
#[class(base=RefCounted, init)]
struct WorkbenchBridge {
    base: Base<RefCounted>,
    pending: Option<mpsc::Receiver<String>>,
    stop: Option<Arc<AtomicBool>>,
}

// Godot JSON 將整數讀成浮點數；只把精確整數還原，絕不取整小數價格／參數。
fn normalize_numbers(value:&mut Value) {
    match value {
        Value::Number(n) if n.is_f64()=>{
            if let Some(f)=n.as_f64() {
                if f.fract()==0.0 && f.abs()<=9_007_199_254_740_991.0 {*value=json!(f as i64);}
            }
        }
        Value::Array(a)=>for v in a {normalize_numbers(v)},
        Value::Object(o)=>for v in o.values_mut(){normalize_numbers(v)},
        _=>{}
    }
}

fn log_message(tx: &mpsc::Sender<String>, line: String) {
    let _ = tx.send(json!({"type":"log","text":line}).to_string());
}

fn apply_chart_market(root: &Path, task: &mut Value, chart: &Value) -> Result<(), String> {
    let required = |key: &str| {
        chart[key].as_str().filter(|text| !text.is_empty()).ok_or("圖表行情設定不完整")
    };
    let symbol = required("symbol")?;
    let database = w::absolute(root, required("database")?);
    let unit = match required("kind")? {
        "second" => "seconds",
        "minute" => "minutes",
        "daily" => "days",
        _ => return Err("圖表回測支援秒 K、分 K 與日 K".into()),
    };
    let period = chart["period"].as_u64().filter(|period| *period > 0).ok_or("圖表週期無效")?;
    let boundary = |key: &str, time: &str| -> Result<String, String> {
        let date = chart[key].as_str().unwrap_or_default();
        if date.is_empty() {
            return Ok(String::new());
        }
        let date = chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d").map_err(|_| "圖表日期無效")?;
        Ok(format!("{date}T{time}"))
    };
    let from = boundary("from", "00:00:00")?;
    let to = boundary("to", "23:59:59")?;
    if !from.is_empty() && !to.is_empty() && from > to {
        return Err("圖表起日不得晚於迄日".into());
    }
    let same_instrument = task["market"]["symbol"].as_str().is_some_and(|saved| saved.eq_ignore_ascii_case(symbol))
        && task["database"].as_str().is_some_and(|saved| w::absolute(root, saved) == database);
    task["database"] = json!(database);
    task["market"] = json!({"symbol":symbol,"unit":unit,"period":period});
    let parameters = &mut task["parameters"];
    parameters["tick_dataset_id"] = Value::Null;
    parameters["from"] = json!(from);
    parameters["to"] = json!(to);
    parameters["bar_anchor_minutes"] = json!(0);
    parameters["timestamp_role"] = json!(if chart["role"] == "start" { "start" } else { "end" });
    if !same_instrument {
        crate::quote_manager::apply_to_task(&database, symbol, unit, task)?;
        if task["market"]["instrument_profile"].is_null() {
            return Err(format!("請先在行情資料庫確認 {symbol} 的最小跳動與每點價值，再執行圖表回測。"));
        }
    }
    Ok(())
}

fn chart_strategy_report(root: &Path, args: &Value) -> Result<Value, String> {
    let source_hash = args["source_hash"].as_str().filter(|hash| !hash.is_empty()).ok_or("找不到策略原碼")?;
    let symbol = args["symbol"].as_str().ok_or("缺少行情商品")?;
    let name = args["name"].as_str().ok_or("缺少策略名稱")?;
    let matches = |receipt: &Value| receipt["status"] == "completed"
        && receipt["source_sha256"] == source_hash
        && receipt["dataset"]["symbol"].as_str().is_some_and(|value| value.eq_ignore_ascii_case(symbol));
    let mut receipt = args["receipt"].clone();
    if !matches(&receipt) {
        let runs = root.join("outputs/pl-workbench");
        if !runs.is_dir() { return Ok(Value::Null); }
        let mut dirs = fs::read_dir(runs).map_err(|e| e.to_string())?
            .filter_map(Result::ok)
            .filter(|item| item.file_type().is_ok_and(|kind| kind.is_dir()))
            .collect::<Vec<_>>();
        dirs.sort_unstable_by_key(|item| std::cmp::Reverse(item.file_name()));
        for dir in dirs {
            let Ok(candidate) = w::read_json(&dir.path().join("result.json")) else { continue };
            if !matches(&candidate) || candidate["run_id"].as_str().is_none_or(|id| dir.file_name() != id) {
                continue;
            }
            let Ok(task) = w::read_json(&dir.path().join("task.json")) else { continue };
            let display = |text: &str| text.trim_start_matches("REA_").trim().to_lowercase();
            if !task["parameters"]["strategy_name"].as_str().is_some_and(|value| display(value) == display(name)) {
                continue;
            }
            if !dir.path().join("engine/report.json").is_file() { continue; }
            receipt = candidate;
            break;
        }
    }
    if !matches(&receipt) { return Ok(Value::Null); }
    #[derive(serde::Deserialize)]
    struct ChartReport { fills: Vec<Value>, #[serde(default)] provenance: Value }
    let path = receipt["artifacts"]["json"].as_str().ok_or("本次回測缺少成交報表")?;
    let input = fs::File::open(path).map_err(|_| "找不到本次回測的成交報表")?;
    // Research reports can contain hundreds of megabytes of equity and event
    // rows. Decode only the fills and provenance needed by the chart.
    let report: ChartReport = serde_json::from_reader(BufReader::new(input))
        .map_err(|e| format!("本次回測的成交報表無法讀取：{e}"))?;
    Ok(json!({"run_id":receipt["run_id"],"fills":report.fills,"zone":report.provenance["timezone"]}))
}

fn execute(mut v:Value,tx:&mpsc::Sender<String>,stop:&AtomicBool)->Result<Value,String> {
    normalize_numbers(&mut v);
    let s=|k:&str|v[k].as_str().ok_or_else(||format!("缺少參數：{k}"));
    let root=Path::new(s("root")?);
    let document=||serde_json::from_value::<lib::Document>(v["document"].clone()).map_err(|e|e.to_string());
    match s("action")? {
        "strategy_requirements" => {
            let mut arguments = json!({"acceptance":v["acceptance"],"request":v["request"]});
            if v["mode"].is_string() {
                arguments["mode"] = v["mode"].clone();
            }
            if let Some(id) = v["entry"]["id"].as_str() {
                arguments["strategy_id"] = json!(id);
            } else if v["document"].is_object() {
                arguments["draft_context"] = crate::strategy_requirements::known_context(&document()?);
            }
            crate::strategy_requirements::query_local_mcp(root, arguments)
        }
        "init" => {
            Ok(json!({"catalog":lib::catalog(root)?,"document":lib::new_document(root,1)?,"limitation":w::LIMITATION}))
        }
        "new"=>{
            if let Some(source) = v["ai_source"].as_str() {
                crate::pl_exchange::validate_generated(source)?;
            }
            Ok(json!(lib::new_document(root,v["number"].as_u64().unwrap_or(1) as usize)?))
        }
        "open"=>{let entry=serde_json::from_value(v["entry"].clone()).map_err(|e|e.to_string())?; Ok(json!(lib::open(root,&entry)?))},
        "import"=>{
            let mut d=lib::new_document(root,1)?;
            let p=Path::new(s("file")?);
            d.source=crate::pl_exchange::read(p)?;
            d.name=p.file_stem().and_then(|x|x.to_str()).unwrap_or("匯入策略").to_owned();
            d.task["parameters"]["strategy_name"]=json!(d.name);d.origin=Some(p.into());
            Ok(json!(d))
        }
        "load_task"=>Ok(w::load_task(root,Path::new(s("file")?))?),
        "save"=>{let mut d=document()?;lib::save(root,&mut d)?;Ok(json!(d))},
        "check"=>check(root,&document()?),
        "strategy_export_pl"=>crate::pl_exchange::export(root,&document()?.source),
        "strategy_diff"=>crate::pl_diff::compare_and_export(root,s("before")?,s("after")?),
        "strategy_logic"=>crate::strategy_logic::analyze(root,&document()?,true),
        "strategy_version"=>crate::strategy_versions::execute(root,document()?,&v),
        "robustness"=>crate::robustness::run(
            root, s("run_id")?, serde_json::from_value(v["options"].clone()).map_err(|e|e.to_string())?,
            |line| log_message(tx,line),
        ),
        "optimize_baseline"=>optimization_run(root,&document()?,None,"godot-rust",|line| log_message(tx,line)),
        "optimize_candidate"=>{
            let baseline=v["baseline"].as_object().ok_or("缺少優化基準")?;
            let mut d=document()?;
            let source=s("candidate")?;
            if source.trim().is_empty() || source.trim()==d.source.trim() {
                return Err("候選版空白或與原版相同。".into());
            }
            crate::pl_exchange::validate_generated(source)?;
            d.source=source.into();
            optimization_run(root,&d,Some(&json!(baseline)),"godot-rust",|line| log_message(tx,line))
        }
        "optimization_draft"=>{
            let mut d=document()?;
            let receipt=&v["receipt"];
            if receipt["status"]!="completed" || !receipt["optimization_inputs"].is_object() {
                return Err("請先完成候選版回測。".into());
            }
            crate::strategy_research::require_trade_validation(receipt)?;
            let source_path=receipt["artifacts"]["source"].as_str().ok_or("缺少候選程式碼")?;
            let task_path=receipt["artifacts"]["task"].as_str().ok_or("缺少候選回測設定")?;
            d.source=fs::read_to_string(source_path).map_err(|e| e.to_string())?;
            let mut frozen=w::read_json(Path::new(task_path))?;
            // 保留原來的行情選擇及報表偏好，使用候選版實際回測的函式快照。
            std::mem::swap(&mut d.task["function_files"], &mut frozen["function_files"]);
            d.id=lib::fresh_id();
            d.name=format!("{}（AI 候選）",d.name);
            d.task["parameters"]["strategy_name"]=json!(d.name);
            d.origin=Some(source_path.into());
            d.managed_id=None;
            d.dirty=true;
            d.result=Some(receipt.clone());
            Ok(json!(d))
        }
        "run" | "run_controlled"=>{
            let mut d=document()?;
            if d.kind!="strategy" {return Err("函式不能單獨回測，請開啟策略".into());}
            if v["action"]=="run_controlled" && !v["chart_market"].is_null() {
                apply_chart_market(root,&mut d.task,&v["chart_market"])?;
            }
            let dest=v["destination"].as_str().filter(|s|!s.trim().is_empty()).map(Path::new);
            let log=|line| log_message(tx,line);
            let mut receipt=if v["action"]=="run_controlled" {
                w::run_controlled(root,d.task,&d.source,log,stop)?
            } else {
                w::run(root,d.task,&d.source,v["excel"].as_bool().unwrap_or(true),dest,log)?
            };
            receipt["ui_runtime"]=json!("godot-rust");
            w::write_json(Path::new(receipt["result_path"].as_str().ok_or("缺少收據路徑")?),&receipt)?;
            view(&receipt,0)
        }
        "view"=>view(&v["receipt"],v["page"].as_u64().unwrap_or(0) as usize),
        "chart_strategy_report"=>chart_strategy_report(root,&v),
        _=>Err("未知工作台操作".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn godot_numeric_roundtrip_retains_fractional_prices_and_integer_config() {
        let mut task=w::read_json(&w::root().join("tasks/pl-workbench-example.json")).unwrap();
        task["parameters"]["bar_anchor_minutes"]=json!(0.0);
        task["parameters"]["warmup_bars"]=json!(500.0);
        task["parameters"]["commission_per_side"]=json!(0.25);
        assert!(serde_json::from_value::<crate::backtest::Config>(task["parameters"].clone()).is_err());
        normalize_numbers(&mut task);
        let config=serde_json::from_value::<crate::backtest::Config>(task["parameters"].clone()).unwrap();
        assert_eq!(config.warmup_bars,500);
        assert_eq!(config.commission_per_side,0.25);
    }
}

#[godot_api]
impl WorkbenchBridge {
    #[func]
    fn start(&mut self,request:GString)->bool {
        if self.pending.is_some(){return false;}
        let request=request.to_string();
        let stop=Arc::new(AtomicBool::new(false));
        self.stop=Some(stop.clone());
        let(tx,rx)=mpsc::channel();self.pending=Some(rx);
        std::thread::spawn(move||{
            let result=std::panic::catch_unwind(||serde_json::from_str(&request).map_err(|e|e.to_string()).and_then(|v|execute(v,&tx,&stop))).unwrap_or_else(|_|Err("背景工作異常中止，請查看本次輸出紀錄".into()));
            let response=match result {Ok(data)=>json!({"type":"done","ok":true,"data":data}),Err(error)=>json!({"type":"done","ok":false,"error":error})};
            let _=tx.send(response.to_string());
        });true
    }
    #[func]
    fn stop_controlled_run(&self) {
        if let Some(stop)=&self.stop {stop.store(true,Ordering::Release);}
    }
    #[func]
    fn poll(&mut self)->GString {
        let Some(rx)=&self.pending else{return GString::new();};
        match rx.try_recv(){
            Ok(text)=>{if text.contains("\"type\":\"done\""){self.pending=None;self.stop=None;}GString::from(&text)},
            Err(mpsc::TryRecvError::Empty)=>GString::new(),
            Err(_)=>{self.pending=None;self.stop=None;GString::from("{\"type\":\"done\",\"ok\":false,\"error\":\"背景工作已中斷\"}")}
        }
    }
}
