//! 桌面回測 HTML 的補充區段，沿用回測收據的統計與檢查快照。
use serde_json::Value;
use std::{
    fs,
    io::{self, Write},
    path::Path,
};

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn cell(value: &Value) -> String {
    match value {
        Value::String(text) => escape(text),
        Value::Number(number) => number.to_string(),
        _ => "—".into(),
    }
}

fn amount(value: &Value) -> String {
    value
        .as_f64()
        .map_or_else(|| "—".into(), |n| format!("{n:.2}"))
}

fn field(out: &mut impl Write, label: &str, value: impl std::fmt::Display) -> io::Result<()> {
    write!(out, "<p><strong>{label}</strong>　{value}</p>")
}

fn periods(out: &mut impl Write, title: &str, periods: &Value) -> io::Result<()> {
    let Some(periods) = periods.as_object() else {
        return Ok(());
    };
    write!(out, "<details><summary>{title}</summary><table><thead><tr><th>期間</th><th>已平倉筆數</th><th>扣費後淨損益</th></tr></thead><tbody>")?;
    for (period, values) in periods {
        write!(
            out,
            "<tr><td>{}</td><td>{}</td><td>{}</td></tr>",
            escape(period),
            cell(&values["trades"]),
            amount(&values["net_pnl"])
        )?;
    }
    write!(out, "</tbody></table></details>")
}

fn supplement(out: &mut impl Write, receipt: &Value) -> io::Result<()> {
    let audit = &receipt["trade_validation"];
    if audit.is_object() {
        let status = if audit["status"] == "passed" {
            "通過"
        } else {
            "未通過，僅供相容性對照"
        };
        field(
            out,
            "逐筆成交驗收",
            format_args!(
                "{}，共檢查 {} 筆，{} 筆違規",
                status,
                cell(&audit["checked_trades"]),
                cell(&audit["violation_count"])
            ),
        )?;
        write!(
            out,
            "<p><a href='trade-validation.json' style='color:#67d9e8'>開啟成交驗收明細</a></p>"
        )?;
    }
    let stats = &receipt["report_statistics"];
    if stats.is_object() {
        write!(out, "<section><h2>交易日績效</h2>")?;
        field(out, "統計日期基準", cell(&stats["trading_day_basis"]))?;
        field(
            out,
            "統計日數與日均交易筆數",
            format_args!(
                "{} 日，每日 {} 筆",
                cell(&stats["trading_days"]),
                amount(&stats["average_daily_trades"])
            ),
        )?;
        if stats["maximum_daily_drawdown"].is_number() {
            field(
                out,
                "最大單日回撤與日期",
                format_args!(
                    "{}（{}）",
                    amount(&stats["maximum_daily_drawdown"]),
                    cell(&stats["maximum_daily_drawdown_date"])
                ),
            )?;
        } else {
            field(
                out,
                "最大單日回撤範圍",
                format_args!(
                    "下界 {}，上界 {}",
                    amount(&stats["maximum_daily_drawdown_lower_bound"]),
                    amount(&stats["maximum_daily_drawdown_upper_bound"])
                ),
            )?;
        }
        periods(out, "月損益", &stats["monthly"])?;
        periods(out, "年損益", &stats["yearly"])?;
        write!(out, "</section>")?;
    }

    let coverage = &receipt["market_coverage"];
    let check = &coverage["last_check"];
    let status = match coverage["status"].as_str() {
        Some("checked") => "檢查範圍涵蓋本次回測",
        Some("stale") => "上次檢查已過期",
        Some("partial_range") => "上次檢查僅涵蓋部分期間",
        Some("out_of_range") => "上次檢查與本次期間無重疊",
        Some("source_mismatch") => "上次檢查對應不同來源或版本",
        Some("unavailable") => "無法讀取檢查紀錄",
        _ => "尚無檢查紀錄",
    };
    write!(
        out,
        "<section><h2>行情完整性</h2><p>以下為回測當時的檢查快照。</p>"
    )?;
    field(
        out,
        "行情來源與資料集",
        format_args!(
            "{}（{}，資料集 {}）",
            cell(&coverage["source"]),
            cell(&coverage["kind"]),
            cell(&coverage["dataset_id"])
        ),
    )?;
    field(out, "行情資料庫", cell(&coverage["database"]))?;
    field(
        out,
        "實際 K 棒起訖（含暖機）",
        format_args!(
            "{} ～ {}",
            datetime(&coverage["first_bar"]),
            datetime(&coverage["last_bar"])
        ),
    )?;
    field(out, "檢查狀態", status)?;
    if check.is_object() {
        field(
            out,
            "最後檢查時間",
            format_args!(
                "{}（{}）{}",
                cell(&check["checked_at"]),
                cell(&check["timezone"]),
                if check["stale"] == true {
                    "，已過期"
                } else {
                    ""
                }
            ),
        )?;
        field(
            out,
            "最後檢查範圍",
            format_args!("{} ～ {}", datetime(&check["from"]), datetime(&check["to"])),
        )?;
    }
    let totals = &check["source_totals"];
    if totals.is_object() {
        field(
            out,
            "檢查全期間的缺漏量",
            format_args!(
                "{}（{}）",
                cell(&totals["missing"]),
                cell(&totals["missing_unit"])
            ),
        )?;
        if let Some(meaning) = totals["meaning"].as_str().filter(|text| !text.is_empty()) {
            field(out, "缺漏定義", escape(meaning))?;
        }
        if let Some(gaps) = coverage["overlapping_gap_examples"]
            .as_array()
            .filter(|gaps| !gaps.is_empty())
        {
            write!(out, "<details><summary>與回測範圍重疊的缺口</summary><ul>")?;
            for gap in gaps {
                write!(
                    out,
                    "<li>{} ～ {}</li>",
                    datetime(&gap["from"]),
                    datetime(&gap["to"])
                )?;
            }
            write!(out, "</ul></details>")?;
        }
        if coverage["gap_examples_truncated"] == true {
            write!(out, "<p>缺口明細為節錄。</p>")?;
        }
    }
    write!(out, "</section>")
}

fn datetime(value: &Value) -> String {
    value.as_str().map_or_else(
        || "—".into(),
        |text| escape(&crate::date_format::display(text)),
    )
}

pub(crate) fn write_html(engine: &Path, target: &Path, receipt: &Value) -> io::Result<()> {
    let original = fs::read_to_string(engine)?;
    let marker = "<h2>交易明細</h2>";
    let (before, after) = original
        .split_once(marker)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "引擎 HTML 缺少交易明細區段"))?;
    let mut out = io::BufWriter::new(
        fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(target)?,
    );
    out.write_all(before.as_bytes())?;
    supplement(&mut out, receipt)?;
    out.write_all(marker.as_bytes())?;
    out.write_all(after.as_bytes())?;
    out.flush()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn shows_stale_coverage_and_escapes_source_as_text() {
        let receipt = json!({"market_coverage": {
            "status":"stale", "source":"測試<script>alert(1)</script>",
            "last_check":{"stale":true,"source_totals":{"missing":2,"missing_unit":"棒數"}},
            "overlapping_gap_examples":[{"from":"2026-01-30T18:03:00","to":"2026-01-30T18:04:00"}],
            "gap_examples_truncated":true
        }});
        let mut html = Vec::new();
        supplement(&mut html, &receipt).unwrap();
        let html = String::from_utf8(html).unwrap();
        assert!(html.contains("上次檢查已過期"));
        assert!(html.contains("2（棒數）"));
        assert!(html.contains("2026/01/30 18:03:00 ～ 2026/01/30 18:04:00"));
        assert!(html.contains("缺口明細為節錄"));
        assert!(html.contains("&lt;script&gt;"));
        assert!(!html.contains("<script>"));
    }
}
