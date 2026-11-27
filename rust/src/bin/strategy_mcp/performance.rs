pub(super) use recharts_core::report_statistics::{performance, DateBasis};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(super) enum Format {
    Html,
    Csv,
}

#[derive(Clone, Copy, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(super) enum Style {
    #[default]
    Full,
    Simple,
}

impl Style {
    fn as_str(self) -> &'static str {
        match self {
            Self::Full => "full",
            Self::Simple => "simple",
        }
    }
}

pub(super) fn new_export(root: &Path) -> Result<PathBuf, String> {
    let base = root.join("outputs/report-exports/mcp");
    fs::create_dir_all(&base).map_err(|e| e.to_string())?;
    let clock = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_nanos();
    for attempt in 0..100 {
        let dest = base.join(format!("{}-{clock}-{attempt}", std::process::id()));
        match fs::create_dir(&dest) {
            Ok(()) => return Ok(dest),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e.to_string()),
        }
    }
    Err("無法建立獨立報告目錄".into())
}

impl Format {
    pub(super) fn extension(self) -> &'static str {
        match self {
            Self::Html => "html",
            Self::Csv => "csv",
        }
    }
}

pub(super) fn export_mc(
    root: &Path,
    run: &Path,
    format: Format,
    style: Style,
    stats: &Value,
    stop: Option<&std::sync::atomic::AtomicBool>,
    log: &mut dyn FnMut(String),
) -> Result<Value, String> {
    use recharts_core::workbench;
    let dir = new_export(root)?;
    let home = std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
        .unwrap_or_default();
    let node = workbench::tool(
        &home,
        "node/bin/node.exe",
        if cfg!(windows) { "node.exe" } else { "node" },
    );
    let supplement = dir.join("performance.json");
    workbench::write_json(&supplement, stats)?;
    workbench::execute_with_stop(
        root,
        &node,
        &[
            "--max-old-space-size=8192".into(),
            root.join("scripts/export-report.mjs"),
            run.join("engine/report.json"),
            dir.join("report.xlsx"),
            "-".into(),
            run.join("input.pl"),
            run.join("task.json"),
            format.extension().into(),
            supplement,
            style.as_str().into(),
        ],
        &dir.join("export.log"),
        log,
        stop,
    )?;
    let target = dir.join(format!("report.{}", format.extension()));
    if !target.is_file() {
        return Err("報表匯出未產生所選格式".into());
    }
    Ok(
        json!({"format":format.extension(),"style":style.as_str(),"path":target,"support_directory":dir,
        "layout_path":dir.join("report.layout.json"),"validation":"共用 MC 欄列、指標計算與對照註記，原生等價範圍依各次回測證據"}),
    )
}

const METRICS: &[(&str, &str)] = &[
    ("gross_pnl", "扣費前損益"),
    ("commission", "手續費"),
    ("net_pnl", "扣費後淨損益"),
    ("trades", "交易筆數"),
    ("average_daily_trades", "日均交易筆數"),
    ("win_rate_pct", "勝率 (%)"),
    ("payoff_ratio", "賺賠比"),
    ("maximum_daily_drawdown", "單日最大回撤"),
    ("maximum_daily_drawdown_upper_bound", "單日最大回撤上界"),
    ("maximum_cumulative_drawdown", "累積最大回撤"),
    ("longest_losing_months", "最長連虧月數"),
];

pub(super) fn compare(
    root: &Path,
    (base_id, base, base_profile): (&str, &Value, &Value),
    (new_id, new, new_profile): (&str, &Value, &Value),
    format: Option<Format>,
    basis: DateBasis,
) -> Result<Value, String> {
    let a = performance(base, base_profile, basis)?;
    let b = performance(new, new_profile, basis)?;
    let mut differences = Vec::new();
    if a["date_basis"] != b["date_basis"] {
        differences.push(json!({"field":"date_basis","baseline":a["date_basis"],"candidate":b["date_basis"]}));
    } else if a["date_basis"] == "trading_date" {
        for key in ["sessions", "trading_weekdays", "closed_dates"] {
            if base_profile[key] != new_profile[key] {
                differences.push(json!({"field":format!("report_calendar.{key}"),"baseline":base_profile[key],"candidate":new_profile[key]}));
            }
        }
    }
    let keys: BTreeSet<_> = [base, new]
        .into_iter()
        .flat_map(|r| r["config"].as_object().into_iter().flat_map(|c| c.keys()))
        .collect();
    for key in keys
        .into_iter()
        .map(String::as_str)
        .filter(|k| !["strategy_name", "research_note", "inputs", "functions"].contains(k))
    {
        if base["config"][key] != new["config"][key] {
            differences.push(
                json!({"field":key,"baseline":base["config"][key],"candidate":new["config"][key]}),
            );
        }
    }
    for key in [
        "symbol",
        "bars_sha256",
        "timezone",
        "kind",
        "source_timestamp_role",
    ] {
        if base["provenance"][key] != new["provenance"][key] || base["provenance"][key].is_null() {
            differences.push(json!({"field":key,"baseline":base["provenance"][key],"candidate":new["provenance"][key]}));
        }
    }
    for key in [
        "source_sha256",
        "verified_block_hashes_sha256",
        "ordering",
        "limit_fill_policy",
    ] {
        if base["tick_execution"][key] != new["tick_execution"][key] {
            differences.push(json!({"field":format!("tick_execution.{key}"),"baseline":base["tick_execution"][key],"candidate":new["tick_execution"][key]}));
        }
    }
    let meta = |id: &str, r: &Value| {
        json!({"run_id":id,"engine_version":r["engine_version"],"engine_sha256":r["engine_sha256"],
        "strategy_sha256":r["strategy_sha256"],"configuration_sha256":r["configuration_sha256"],"inputs":r["inputs"]})
    };
    let delta = |x: &Value, y: &Value| y.as_f64().zip(x.as_f64()).map(|(y, x)| y - x);
    let metrics:Vec<_>=METRICS.iter().map(|(key,label)|json!({"key":key,"label":label,"baseline":a[*key],"candidate":b[*key],"delta":delta(&a[*key],&b[*key])})).collect();
    let align = |group: &str| {
        let a = a[group].as_object().unwrap();
        let b = b[group].as_object().unwrap();
        let keys: BTreeSet<_> = a.keys().chain(b.keys()).collect();
        keys.into_iter().map(|k| {
            let x=a.get(k).cloned().unwrap_or_default();let y=b.get(k).cloned().unwrap_or_default();
            json!({"period":k,"baseline":x,"candidate":y,"net_delta":delta(&x["net_pnl"],&y["net_pnl"])})
        }).collect::<Vec<_>>()
    };
    let mut output = json!({"schema":"model-update-comparison-v1","baseline":meta(base_id,base),"candidate":meta(new_id,new),
        "date_basis":{"baseline":a["date_basis"],"candidate":b["date_basis"]},
        "date_basis_description":{"baseline":a["trading_day_basis"],"candidate":b["trading_day_basis"]},
        "same_data_and_conditions":differences.is_empty(),"comparison_differences":differences,
        "metrics":metrics,"yearly":align("yearly"),"monthly":align("monthly")});
    if let Some(format) = format {
        let dir = new_export(root)?;
        let file = dir.join(format!("model-update.{}", format.extension()));
        write_comparison(&file, format, &output)?;
        output["export"] = json!({"format":format.extension(),"path":file});
    }
    Ok(output)
}

fn text(v: &Value) -> String {
    match v {
        Value::Null => "—".into(),
        Value::String(s) => s.clone(),
        _ => v.to_string(),
    }
}
pub(super) fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}
fn write_comparison(path: &Path, format: Format, report: &Value) -> Result<(), String> {
    let mut rows: Vec<Vec<String>> = vec![vec![
        "區段".into(),
        "項目".into(),
        "更新前".into(),
        "更新後".into(),
        "差值（更新後減更新前）".into(),
    ]];
    let row = |section: &str, label: &str, a: &Value, b: &Value, d: &Value| {
        vec![section.into(), label.into(), text(a), text(b), text(d)]
    };
    let comparable = report["same_data_and_conditions"] == true;
    let status = if comparable {
        "行情與指定交易設定一致"
    } else {
        "行情或交易設定有差異，數值差值僅供對照"
    };
    rows.push(vec![
        "說明".into(),
        status.into(),
        String::new(),
        String::new(),
        String::new(),
    ]);
    rows.push(row("說明", "統計日期基準", &report["date_basis_description"]["baseline"],
        &report["date_basis_description"]["candidate"], &Value::Null));
    for (key, label) in [
        ("run_id", "回測編號"),
        ("strategy_sha256", "策略 SHA-256"),
        ("engine_version", "引擎版本"),
        ("engine_sha256", "引擎 SHA-256"),
        ("configuration_sha256", "設定 SHA-256"),
        ("inputs", "策略輸入"),
    ] {
        rows.push(row(
            "識別",
            label,
            &report["baseline"][key],
            &report["candidate"][key],
            &Value::Null,
        ));
    }
    for v in report["comparison_differences"].as_array().unwrap() {
        rows.push(row(
            "條件差異",
            v["field"].as_str().unwrap_or_default(),
            &v["baseline"],
            &v["candidate"],
            &Value::Null,
        ));
    }
    for v in report["metrics"].as_array().unwrap() {
        rows.push(row(
            "損益摘要",
            v["label"].as_str().unwrap_or_default(),
            &v["baseline"],
            &v["candidate"],
            &v["delta"],
        ));
    }
    for (key, label) in [("yearly", "逐年淨損益"), ("monthly", "逐月淨損益")] {
        for v in report[key].as_array().unwrap() {
            rows.push(row(
                label,
                v["period"].as_str().unwrap_or_default(),
                &v["baseline"]["net_pnl"],
                &v["candidate"]["net_pnl"],
                &v["net_delta"],
            ));
        }
    }
    match format {
        Format::Csv => {
            use std::io::Write;
            let mut f = fs::File::create(path).map_err(|e| e.to_string())?;
            f.write_all(b"\xef\xbb\xbf").map_err(|e| e.to_string())?;
            let mut w = csv::Writer::from_writer(f);
            for r in rows {
                // Text beginning with spreadsheet formula operators must remain text.
                let safe = r.iter().map(|s| {
                    if s.trim_start().starts_with(['=', '+', '-', '@']) && s.parse::<f64>().is_err()
                    {
                        format!("'{s}")
                    } else {
                        s.clone()
                    }
                });
                w.write_record(safe).map_err(|e| e.to_string())?;
            }
            w.flush().map_err(|e| e.to_string())
        }
        Format::Html => {
            let mut html=String::from("<!doctype html><html lang='zh-Hant'><meta charset='utf-8'><title>模型更新比較報告</title><style>body{font:15px system-ui;margin:2rem}table{border-collapse:collapse}td,th{border:1px solid #ccd6e1;padding:.5rem}</style><h1>模型更新比較報告</h1><table>");
            for (i, r) in rows.iter().enumerate() {
                let tag = if i == 0 { "th" } else { "td" };
                html.push_str("<tr>");
                for cell in r {
                    html.push_str(&format!("<{tag}>{}</{tag}>", escape(cell)));
                }
                html.push_str("</tr>");
            }
            html.push_str("</table></html>");
            fs::write(path, html).map_err(|e| e.to_string())
        }
    }
}
