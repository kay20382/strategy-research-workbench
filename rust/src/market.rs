use chrono::{Datelike, Duration, NaiveDate, Weekday};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Bar {
    pub date: String,
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
    pub volume: f64,
}

#[derive(Serialize)]
pub struct StudyBar {
    #[serde(flatten)]
    pub bar: Bar,
    pub ma5: Option<f64>,
    pub ma20: Option<f64>,
    pub ma60: Option<f64>,
    pub roc: Option<f64>,
}

pub fn enrich(bars: &[Bar]) -> Vec<StudyBar> {
    bars.iter().enumerate().map(|(i, bar)| {
        let sma = |n: usize| if i + 1 >= n {
            Some(bars[i + 1 - n..=i].iter().map(|b| b.close).sum::<f64>() / n as f64)
        } else { None };
        StudyBar { bar: bar.clone(), ma5: sma(5), ma20: sma(20), ma60: sma(60),
            roc: if i >= 10 && bars[i - 10].close != 0.0 {
                Some((bar.close / bars[i - 10].close - 1.0) * 100.0)
            } else { None } }
    }).collect()
}

fn check(bar: &Bar) -> Result<NaiveDate, String> {
    let date = NaiveDate::parse_from_str(&bar.date, "%Y-%m-%d")
        .map_err(|_| format!("日期須為 YYYY-MM-DD：{}", bar.date))?;
    if ![bar.open, bar.high, bar.low, bar.close, bar.volume].iter().all(|x| x.is_finite()) {
        return Err(format!("{} 含有無效數值", bar.date));
    }
    if bar.volume < 0.0 || bar.high < bar.low || bar.high < bar.open.max(bar.close)
        || bar.low > bar.open.min(bar.close) {
        return Err(format!("{} 的最高價、最低價或成交量不合理", bar.date));
    }
    Ok(date)
}

pub fn parse_csv(text: &str) -> Result<Vec<Bar>, String> {
    let text = text.trim_start_matches('\u{feff}');
    let mut reader = csv::ReaderBuilder::new().trim(csv::Trim::All).from_reader(text.as_bytes());
    let headers: csv::StringRecord = reader.headers().map_err(|e| e.to_string())?.iter()
        .map(|h| match h.to_ascii_lowercase().as_str() {
            "totalvolume" => "volume".to_string(),
            other => other.to_string(),
        }).collect();
    reader.set_headers(headers.clone());
    let required = ["date", "open", "high", "low", "close", "volume"];
    for name in required {
        if !headers.iter().any(|h| h == name) {
            return Err(format!("CSV 缺少欄位 {name}；需含 date,open,high,low,close,volume"));
        }
    }
    let mut sorted = BTreeMap::new();
    for (index, record) in reader.deserialize::<Bar>().enumerate() {
        let mut bar = record.map_err(|e| format!("第 {} 列：{e}", index + 2))?;
        if bar.date.contains('/') {
            bar.date = NaiveDate::parse_from_str(&bar.date, "%Y/%m/%d")
                .map_err(|_| format!("日期格式不正確：{}",bar.date))?.to_string();
        }
        let date = check(&bar)?;
        if sorted.insert(date, bar).is_some() { return Err(format!("日期重複：{date}")); }
    }
    if sorted.is_empty() { return Err("CSV 沒有行情資料".into()); }
    Ok(sorted.into_values().collect())
}

pub fn aggregate(bars: &[Bar], period: i64) -> Result<Vec<Bar>, String> {
    if ![0, 1, 2].contains(&period) { return Err("不支援的週期".into()); }
    let mut result: Vec<Bar> = Vec::new();
    let mut last_key = String::new();
    let mut previous: Option<NaiveDate> = None;
    for bar in bars {
        let date = check(bar)?;
        if previous.is_some_and(|p| date <= p) { return Err("行情日期須遞增且不可重複".into()); }
        previous = Some(date);
        let key = match period {
            1 => format!("{}-{}", date.iso_week().year(), date.iso_week().week()),
            2 => format!("{}-{}", date.year(), date.month()),
            _ => bar.date.clone(),
        };
        if key == last_key {
            let current = result.last_mut().unwrap();
            current.high = current.high.max(bar.high);
            current.low = current.low.min(bar.low);
            current.close = bar.close;
            current.volume += bar.volume;
        } else { result.push(bar.clone()); last_key = key; }
    }
    Ok(result)
}

pub fn demo(seed: u64) -> Vec<Bar> {
    let mut state = seed.wrapping_add(93719);
    let mut random = || { state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
        ((state >> 33) as f64) / ((1u64 << 31) as f64) };
    let mut date = NaiveDate::from_ymd_opt(2025, 9, 1).unwrap();
    let end = NaiveDate::from_ymd_opt(2026, 10, 23).unwrap();
    let mut result = Vec::new();
    let mut close = 26700.0;
    let mut i: f64 = 0.0;
    while date <= end {
        if !matches!(date.weekday(), Weekday::Sat | Weekday::Sun) {
            let open = close + (random() - 0.5) * 180.0;
            let drift = 43.0 + (i / 23.0).sin() * 96.0;
            close = (open + drift + (random() - 0.47) * 580.0).max(1000.0);
            result.push(Bar { date: date.to_string(), open, close,
                high: open.max(close) + random() * 230.0,
                low: open.min(close) - random() * 230.0,
                volume: (65000.0 + random() * 180000.0 + (close - open).abs() * 220.0).round() });
            i += 1.0;
        }
        date += Duration::days(1);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    const CSV: &str = "date,open,high,low,close,volume\n2026-01-05,10,14,9,12,100\n2026-01-06,12,16,11,15,200\n2026-01-12,15,19,14,18,300\n";
    #[test] fn weekly_ohlcv() {
        let bars = aggregate(&parse_csv(CSV).unwrap(), 1).unwrap();
        assert_eq!(bars.len(), 2);
        assert_eq!((bars[0].open,bars[0].high,bars[0].low,bars[0].close,bars[0].volume), (10.0,16.0,9.0,15.0,300.0));
    }
    #[test] fn monthly_ohlcv() {
        let bars = aggregate(&parse_csv(CSV).unwrap(), 2).unwrap();
        assert_eq!(bars.len(), 1); assert_eq!(bars[0].volume, 600.0); assert_eq!(bars[0].close,18.0);
    }
    #[test] fn rejects_invalid_ohlc_and_duplicates() {
        assert!(parse_csv(&CSV.replace(",14,9,", ",8,9,")).is_err());
        assert!(parse_csv(&format!("{CSV}2026-01-05,10,14,9,12,100\n")).is_err());
        assert!(parse_csv(&CSV.replace(",100", ",-100")).is_err());
        assert!(parse_csv(&CSV.replace(",12,100", ",NaN,100")).is_err());
    }
    #[test] fn sorts_and_accepts_bom() {
        let input = "\u{feff}date,open,high,low,close,volume\n2026-01-06,12,16,11,15,200\n2026-01-05,10,14,9,12,100\n";
        assert_eq!(parse_csv(input).unwrap()[0].date,"2026-01-05");
    }
    #[test] fn weekly_year_boundary() {
        let input = CSV.replace("2026-01-05","2025-12-31").replace("2026-01-06","2026-01-01").replace("2026-01-12","2026-01-05");
        assert_eq!(aggregate(&parse_csv(&input).unwrap(),1).unwrap().len(),2);
    }
    #[test] fn study_warmup_and_values() {
        let mut bars = parse_csv(CSV).unwrap();
        for _ in 0..9 { bars.push(bars.last().unwrap().clone()); }
        let values=enrich(&bars);
        assert!(values[3].ma5.is_none()); assert_eq!(values[4].ma5,Some(16.2));
        assert!(values[9].roc.is_none()); assert_eq!(values[10].roc,Some(50.0));
    }
    #[test] fn demo_is_valid_and_repeatable() {
        let bars=demo(42); assert!(bars.len()>250); assert!(bars.iter().all(|b|check(b).is_ok()));
        assert_eq!(bars[10].close,demo(42)[10].close); assert_ne!(bars[10].close,demo(43)[10].close);
    }
    #[test] fn multicharts_export_headers_and_dates() {
        let bars=parse_csv("\"Date\",\"Open\",\"High\",\"Low\",\"Close\",\"TotalVolume\"\n2024/6/18,22588,22812,22586,22786,100257\n").unwrap();
        assert_eq!(bars[0].date,"2024-06-18"); assert_eq!(bars[0].volume,100257.0);
    }
    #[test] fn supplied_txf_file_is_preserved() {
        let path=std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../TXF1-Day-Trade.txt");
        if !path.exists() { return; }
        let bars=parse_csv(&std::fs::read_to_string(path).unwrap()).unwrap();
        assert_eq!(bars.len(),657);
        assert_eq!(bars.first().unwrap().date,"2024-06-18");
        let last=bars.last().unwrap();
        assert_eq!(last.date,"2026-10-23"); assert_eq!(last.close,45654.0); assert_eq!(last.volume,39159.0);
        assert!(bars.iter().any(|b| b.date=="2024-06-22")); // Preserve source dates, including Saturdays.
    }
}

