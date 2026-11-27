use recharts_core::{
    backtest::{self, Config, ResultReport},
    powerlanguage::Program,
};
use serde::Deserialize;
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Job {
    database: String,
    strategy: String,
    output: String,
    function_files: BTreeMap<String, String>,
    parameters: Config,
    #[serde(default)]
    instrument_specifications: Option<String>,
    #[serde(default, rename = "report_settings")]
    _report_settings: BTreeMap<String, serde_json::Value>,
    #[serde(default, rename = "market")]
    _market: Option<serde_json::Value>,
}
#[derive(Deserialize)]
struct InstrumentSpecification {
    price_scale:f64,
    minimum_move:u64,
    tick_size:f64,
    point_value:f64,
}
#[derive(Deserialize)]
struct InstrumentSpecifications {instruments:BTreeMap<String,InstrumentSpecification>}

fn apply_instrument_specification(cfg:&mut Config,symbol:&str,registry:&InstrumentSpecifications)->Result<(),String> {
    // 明示設定必須完整；不要用商品表補上另一半而產生未經確認的組合。
    if cfg.price_scale.is_some() || cfg.minimum_move.is_some() {return Ok(());}
    let specification=registry.instruments.iter().find(|(name,_)|name.eq_ignore_ascii_case(symbol)).map(|(_,s)|s)
        .ok_or_else(||format!("商品 {symbol} 尚無共用價格規格，請提供實際 PriceScale 與 MinMove"))?;
    if (cfg.tick_size-specification.tick_size).abs()>specification.tick_size*1e-12
        || (cfg.point_value-specification.point_value).abs()>specification.point_value*1e-12 {
        return Err(format!("商品 {symbol} 的任務跳動／每點價值與共用商品規格不同，未自動覆蓋"));
    }
    cfg.price_scale=Some(specification.price_scale);cfg.minimum_move=Some(specification.minimum_move);
    Ok(())
}
fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}
fn write_reports(dir: &Path, r: &ResultReport) -> Result<(), String> {
    fs::write(
        dir.join("report.json"),
        serde_json::to_vec_pretty(r).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    let mut csv = csv::Writer::from_path(dir.join("trades.csv")).map_err(|e| e.to_string())?;
    if r.trades.is_empty() {
        csv.write_record([
            "id",
            "entry_order_id",
            "exit_order_id",
            "side",
            "quantity",
            "entry_bar",
            "exit_bar",
            "entry_datetime",
            "exit_datetime",
            "entry_label",
            "exit_label",
            "entry_price",
            "exit_price",
            "gross",
            "costs",
            "net",
            "mfe",
            "mae",
            "mfe_gross",
            "mae_gross",
            "cumulative",
        ])
        .map_err(|e| e.to_string())?;
    }
    for t in &r.trades {
        let mut shown=t.clone();
        shown.entry_datetime=recharts_core::date_format::display(&t.entry_datetime);
        shown.exit_datetime=recharts_core::date_format::display(&t.exit_datetime);
        csv.serialize(shown).map_err(|e| e.to_string())?;
    }
    csv.flush().map_err(|e| e.to_string())?;
    let mut equity = csv::Writer::from_path(dir.join("equity.csv")).map_err(|e| e.to_string())?;
    for x in &r.equity {
        let mut shown=x.clone();shown.datetime=recharts_core::date_format::display(&x.datetime);
        equity.serialize(shown).map_err(|e| e.to_string())?;
    }
    equity.flush().map_err(|e| e.to_string())?;
    let s = &r.summary;
    let warnings = r
        .warnings
        .iter()
        .map(|w| format!("<li>{}</li>", escape(w)))
        .collect::<String>();
    let rows=r.trades.iter().map(|t|format!("<tr><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{:.0}</td><td>{:.0}</td><td>{:.2}</td><td>{:.2}</td><td>{:.2}</td></tr>",t.id,if t.side>0{"多"}else{"空"},escape(&recharts_core::date_format::display(&t.entry_datetime)),escape(&recharts_core::date_format::display(&t.exit_datetime)),escape(&t.exit_label),t.entry_price,t.exit_price,t.costs,t.net,t.cumulative)).collect::<String>();
    let html=format!("<!doctype html><html lang='zh-Hant'><meta charset='utf-8'><title>策略回測報告</title><style>body{{font:15px system-ui;background:#0b101b;color:#e2e8f0;margin:32px}}h1,h2{{color:#67d9e8}}.cards{{display:flex;flex-wrap:wrap;gap:20px}}.card{{background:#172437;padding:18px;border-radius:10px}}b{{font-size:24px;display:block}}table{{border-collapse:collapse;width:100%}}td,th{{padding:8px;border-bottom:1px solid #26364c;text-align:right}}th{{position:sticky;top:0;background:#172437}}li{{margin:8px 0}}code{{word-break:break-all}}</style><h1>策略回測報告</h1><p>研究引擎；非 MC 完整等價驗證</p><p>{}</p><div class='cards'><div class='card'>已平倉淨利<b>{:.2}</b></div><div class='card'>含未平倉淨損益<b>{:.2}</b></div><div class='card'>成交交易數<b>{}</b></div><div class='card'>最大假設路徑回撤<b>{:.2}</b></div><div class='card'>單邊成本合計<b>{:.2}</b></div></div><h2>資料與假設</h2><p>{} 至 {}；{} 根 K 棒；歧義棒 {} 根</p><p>行情雜湊 <code>{}</code></p><p>策略雜湊 <code>{}</code></p><ul>{}</ul><h2>交易明細</h2><table><thead><tr><th>編號</th><th>方向</th><th>進場棒時間</th><th>出場棒時間</th><th>出場訊號</th><th>進價</th><th>出價</th><th>成本</th><th>淨損益</th><th>累積損益</th></tr></thead><tbody>{}</tbody></table></html>",escape(&r.config.research_note),s.net_closed,s.net_total,s.trades,s.max_drawdown,s.commissions,escape(&recharts_core::date_format::display(&r.first_bar)),escape(&recharts_core::date_format::display(&r.last_bar)),r.bar_count,r.ambiguous_bars,r.provenance.bars_sha256,r.strategy_sha256,warnings,rows);
    fs::write(dir.join("report.html"), html).map_err(|e| e.to_string())?;
    Ok(())
}
fn run() -> Result<(), String> {
    let args: Vec<_> = std::env::args().collect();
    if !(2..=3).contains(&args.len()) {
        return Err("用法：backtest 任務.json [新的輸出目錄]；相對路徑以目前工作目錄為準".into());
    }
    let mut job: Job =
        serde_json::from_str(&fs::read_to_string(&args[1]).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
    if job.parameters.price_scale.is_none() && job.parameters.minimum_move.is_none() {
        let connection=recharts_core::database::open_existing(Path::new(&job.database))?;
        let symbol:String=connection.query_row("SELECT symbol FROM datasets WHERE id=?1",[job.parameters.dataset_id],|r|r.get(0)).map_err(|e|e.to_string())?;
        let file=job.instrument_specifications.as_deref().unwrap_or("data/instrument-specifications.json");
        let registry:InstrumentSpecifications=serde_json::from_str(&fs::read_to_string(file).map_err(|e|format!("商品規格 {file}: {e}"))?).map_err(|e|e.to_string())?;
        apply_instrument_specification(&mut job.parameters,&symbol,&registry)?;
    }
    if !job.parameters.functions.is_empty() && !job.function_files.is_empty() {
        return Err("functions 與 function_files 不可同時提供".into());
    }
    for (name, path) in job.function_files {
        if job.parameters.functions.contains_key(&name.to_ascii_lowercase()) {
            return Err(format!("函式檔案名稱不分大小寫重複：{name}"));
        }
        job.parameters.functions.insert(
            name.to_ascii_lowercase(),
            fs::read_to_string(path).map_err(|e| e.to_string())?,
        );
    }
    let source = fs::read_to_string(&job.strategy).map_err(|e| e.to_string())?;
    if job.parameters.strategy_name.is_empty() {
        job.parameters.strategy_name = Path::new(&job.strategy)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("策略")
            .split('.')
            .next()
            .unwrap_or("策略")
            .into();
    }
    let program =
        Program::with_functions(&source, &job.parameters.inputs, &job.parameters.functions)?;
    let output = PathBuf::from(args.get(2).unwrap_or(&job.output));
    if output.exists() {
        return Err("輸出目錄已存在；請指定新的目錄，避免覆寫研究結果".into());
    }
    eprintln!("策略語法檢查完成，從資料庫載入行情…");
    let (bars, provenance) = backtest::load(Path::new(&job.database), &job.parameters)?;
    eprintln!(
        "已讀取 {} 筆來源資料，彙整 {} 根 K 棒，執行回測…",
        provenance.source_rows,
        bars.len()
    );
    let report = if job.parameters.tick_dataset_id.is_some() {
        backtest::run_with_database_ticks(Path::new(&job.database), &bars, &program, &job.parameters, provenance, &source)?
    } else { backtest::run(&bars, &program, &job.parameters, provenance, &source)? };
    let parent = output.parent().ok_or("輸出目錄錯誤")?;
    fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let staging = parent.join(format!(".backtest-{}", std::process::id()));
    fs::create_dir(&staging).map_err(|e| e.to_string())?;
    write_reports(&staging, &report)?;
    // 報表圖形只使用本次實際載入、含暖機的行情，不沿用其他任務的 CSV。
    let mut candles = csv::Writer::from_path(staging.join("bars.csv")).map_err(|e| e.to_string())?;
    for bar in &bars {
        candles.serialize(bar).map_err(|e| e.to_string())?;
    }
    candles.flush().map_err(|e| e.to_string())?;
    drop(candles);
    fs::write(staging.join("strategy.pl"), &source).map_err(|e| e.to_string())?;
    fs::rename(&staging, &output).map_err(|e| e.to_string())?;
    println!(
        "{}",
        serde_json::json!({"output":output,"summary":report.summary,"warnings":report.warnings,"bars":report.bar_count,"ambiguous_bars":report.ambiguous_bars})
    );
    Ok(())
}
fn main() {
    if let Err(e) = run() {
        eprintln!("回測失敗：{e}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn symbol_specification_is_shared_and_does_not_guess_from_tick_size() {
        let task:serde_json::Value=serde_json::from_str(include_str!("../../../tasks/kgi-first.json")).unwrap();
        let template:Config=serde_json::from_value(task["parameters"].clone()).unwrap();
        let registry:InstrumentSpecifications=serde_json::from_str(include_str!("../../../data/instrument-specifications.json")).unwrap();
        for name in ["NH14","YAO1","KAIv7","其他策略"] {
            let mut c=template.clone();c.strategy_name=name.into();
            apply_instrument_specification(&mut c,"TXF1",&registry).unwrap();
            assert_eq!(c.price_scale,Some(1.0));assert_eq!(c.minimum_move,Some(1));
        }
        let mut missing=template.clone();assert!(apply_instrument_specification(&mut missing,"未登錄商品",&registry).is_err());
        let mut conflicting=template.clone();conflicting.tick_size=0.25;
        assert!(apply_instrument_specification(&mut conflicting,"TXF1",&registry).is_err());
        let mut explicit=template;explicit.price_scale=Some(100.0);explicit.minimum_move=Some(25);
        apply_instrument_specification(&mut explicit,"未登錄商品",&registry).unwrap();
        assert_eq!(explicit.price_scale,Some(100.0));assert_eq!(explicit.minimum_move,Some(25));
    }
}
