//! Streaming trade ticks to OHLCV. Fixed-point arithmetic; equal timestamps retain input order.
use chrono::{NaiveDateTime, Timelike};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::{BufReader, Read},
    path::Path,
};

const SCALE: i128 = 1_000_000;
pub(crate) fn decimal(s: &str) -> Result<i128, String> {
    let s = s.trim();
    let negative = s.starts_with('-');
    let raw = s
        .strip_prefix('-')
        .or_else(|| s.strip_prefix('+'))
        .unwrap_or(s);
    let mut parts = raw.split('.');
    let whole = parts.next().unwrap_or("");
    let fraction = parts.next().unwrap_or("");
    if parts.next().is_some()
        || whole.is_empty()
        || !whole.bytes().all(|c| c.is_ascii_digit())
        || !fraction.bytes().all(|c| c.is_ascii_digit())
        || fraction.len() > 6
    {
        return Err(format!("數值須為最多六位小數：{s}"));
    }
    let a = whole.parse::<i128>().map_err(|_| "數值溢位")?;
    let b = if fraction.is_empty() {
        0
    } else {
        fraction.parse::<i128>().map_err(|_| "小數格式錯誤")?
            * 10i128.pow(6 - fraction.len() as u32)
    };
    let n = a
        .checked_mul(SCALE)
        .and_then(|a| a.checked_add(b))
        .ok_or("數值溢位")?;
    Ok(if negative { -n } else { n })
}
pub(crate) fn text(n: i128) -> String {
    let sign = if n < 0 { "-" } else { "" };
    let n = n.unsigned_abs();
    let whole = n / SCALE as u128;
    let fraction = n % SCALE as u128;
    if fraction == 0 {
        format!("{sign}{whole}")
    } else {
        format!(
            "{sign}{whole}.{}",
            format!("{fraction:06}").trim_end_matches('0')
        )
    }
}
#[derive(Debug, Clone, Serialize)]
pub struct TickBar {
    pub datetime: String,
    pub available_at: String,
    pub open: String,
    pub high: String,
    pub low: String,
    pub close: String,
    pub volume: String,
    pub tick_count: u64,
}
struct Active {
    bucket: i64,
    end: i64,
    open: i128,
    high: i128,
    low: i128,
    close: i128,
    volume: i128,
    count: u64,
}
pub struct Aggregator {
    seconds: i64,
    anchor: i64,
    previous: Option<NaiveDateTime>,
    active: Option<Active>,
    sessions: bool,
    night_start: i64,
    close_exclusive: bool,
}
impl Aggregator {
    pub fn new(seconds: i64, anchor: i64) -> Result<Self, String> {
        if !(1..=86400).contains(&seconds) || !(0..seconds).contains(&anchor) {
            return Err("週期須為 1～86400 秒，對齊位移須在 0 至週期減一之間".into());
        }
        Ok(Self {
            seconds,
            anchor,
            previous: None,
            active: None,
            sessions: false,
            night_start: 15 * 3600,
            close_exclusive: false,
        })
    }
    pub fn with_sessions(seconds: i64) -> Result<Self, String> {
        let mut a = Self::new(seconds, 0)?;
        a.sessions = true;
        Ok(a)
    }
    pub(crate) fn with_stock_sessions(seconds: i64) -> Result<Self, String> {
        let mut a = Self::with_sessions(seconds)?;
        a.night_start = 17 * 3600 + 25 * 60;
        Ok(a)
    }
    pub fn with_sessions_close_exclusive(seconds: i64) -> Result<Self, String> {
        let mut a = Self::with_sessions(seconds)?;
        a.close_exclusive = true;
        Ok(a)
    }
    pub(crate) fn window(&self, dt: NaiveDateTime) -> Option<(i64, i64)> {
        let timestamp = dt.and_utc().timestamp();
        if !self.sessions {
            let start =
                (timestamp - self.anchor).div_euclid(self.seconds) * self.seconds + self.anchor;
            return Some((start, start + self.seconds));
        }
        let midnight = dt.date().and_hms_opt(0, 0, 0)?.and_utc().timestamp();
        let clock = dt.num_seconds_from_midnight() as i64;
        let (start, end) = if clock >= self.night_start {
            (midnight + self.night_start, midnight + 86400 + 18000)
        } else if clock <= 18000 {
            (midnight - 86400 + self.night_start, midnight + 18000)
        } else if (31500..=49500).contains(&clock) {
            (midnight + 31500, midnight + 49500)
        } else {
            return None;
        };
        if timestamp == end && (self.close_exclusive || dt.nanosecond() > 0) {
            return None;
        }
        // Retain the session-close print in the final bar instead of creating a post-close bar.
        let effective = if timestamp == end {
            timestamp - 1
        } else {
            timestamp
        };
        let bucket = start + (effective - start).div_euclid(self.seconds) * self.seconds;
        Some((bucket, (bucket + self.seconds).min(end)))
    }
    fn finish_active(&mut self) -> Result<Option<TickBar>, String> {
        self.active
            .take()
            .map(|a| {
                let end = a.end;
                let dt = chrono::DateTime::from_timestamp(end, 0)
                    .ok_or("時間超出可表示範圍")?
                    .naive_utc()
                    .format("%Y-%m-%dT%H:%M:%S")
                    .to_string();
                Ok(TickBar {
                    datetime: dt.clone(),
                    available_at: dt,
                    open: text(a.open),
                    high: text(a.high),
                    low: text(a.low),
                    close: text(a.close),
                    volume: text(a.volume),
                    tick_count: a.count,
                })
            })
            .transpose()
    }
    pub fn push(
        &mut self,
        dt: NaiveDateTime,
        price: &str,
        volume: &str,
    ) -> Result<Option<TickBar>, String> {
        self.push_fixed(dt, decimal(price)?, decimal(volume)?)
    }
    /// Values use the canonical fixed6 scale; no float round trip.
    pub fn push_fixed(&mut self, dt: NaiveDateTime, price: i128, volume: i128) -> Result<Option<TickBar>, String> {
        if self.previous.is_some_and(|p| dt < p) {
            return Err("Tick 時間倒退；不能在串流彙整中偷偷重排或遺漏資料".into());
        }
        if volume < 0 {
            return Err("成交量不得為負".into());
        }
        let (bucket, end) = self.window(dt).ok_or("Tick 不在指定的日夜盤時段")?;
        let completed = if self.active.as_ref().is_some_and(|a| a.bucket != bucket) {
            self.finish_active()?
        } else {
            None
        };
        if let Some(a) = &mut self.active {
            a.high = a.high.max(price);
            a.low = a.low.min(price);
            a.close = price;
            a.volume = a.volume.checked_add(volume).ok_or("成交量加總溢位")?;
            a.count = a.count.checked_add(1).ok_or("成交筆數溢位")?
        } else {
            self.active = Some(Active {
                bucket,
                end,
                open: price,
                high: price,
                low: price,
                close: price,
                volume,
                count: 1,
            })
        }
        self.previous = Some(dt);
        Ok(completed)
    }
    pub fn finish(&mut self) -> Result<Option<TickBar>, String> {
        self.finish_active()
    }
}
struct Hashed<R> {
    reader: R,
    hash: Sha256,
}
impl<R: Read> Read for Hashed<R> {
    fn read(&mut self, b: &mut [u8]) -> std::io::Result<usize> {
        let n = self.reader.read(b)?;
        self.hash.update(&b[..n]);
        Ok(n)
    }
}
#[derive(Serialize)]
pub struct Summary {
    pub source: String,
    pub source_bytes: u64,
    pub source_sha256: String,
    pub seconds: i64,
    pub anchor_seconds: i64,
    pub alignment: String,
    pub sessions: Vec<String>,
    pub timestamp_role: String,
    pub interval_rule: String,
    pub empty_intervals: String,
    pub timezone: String,
    pub input_ticks: u64,
    pub aggregated_ticks: u64,
    pub outside_session_ticks: u64,
    pub output_bars: u64,
    pub total_volume: String,
    pub aggregated_volume: String,
    pub outside_session_volume: String,
    pub first_tick: String,
    pub last_tick: String,
    pub last_bar_may_be_partial: bool,
}
pub fn convert(input: &Path, output: &Path, seconds: i64, anchor: i64) -> Result<Summary, String> {
    convert_mode(input, output, seconds, anchor, false)
}
pub fn convert_sessions(input: &Path, output: &Path, seconds: i64) -> Result<Summary, String> {
    convert_mode(input, output, seconds, 0, true)
}
fn convert_mode(
    input: &Path,
    output: &Path,
    seconds: i64,
    anchor: i64,
    sessions: bool,
) -> Result<Summary, String> {
    let mut aggregator = if sessions {
        Aggregator::with_sessions(seconds)?
    } else {
        Aggregator::new(seconds, anchor)?
    };
    if output.exists() {
        return Err("輸出目錄已存在；請使用新目錄".into());
    }
    let source = File::open(input).map_err(|e| e.to_string())?;
    let bytes = source.metadata().map_err(|e| e.to_string())?.len();
    fs::create_dir_all(output).map_err(|e| e.to_string())?;
    let part = output.join("bars.csv.partial");
    let target = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&part)
        .map_err(|e| e.to_string())?;
    let mut writer = csv::Writer::from_writer(target);
    let hash = Hashed {
        reader: BufReader::with_capacity(4 * 1024 * 1024, source),
        hash: Sha256::new(),
    };
    let mut reader = csv::ReaderBuilder::new()
        .trim(csv::Trim::All)
        .from_reader(hash);
    let headers = reader.headers().map_err(|e| e.to_string())?.clone();
    let columns = headers
        .iter()
        .map(|h| h.trim_start_matches('\u{feff}').to_ascii_lowercase())
        .collect::<Vec<_>>();
    if columns
        .iter()
        .collect::<std::collections::HashSet<_>>()
        .len()
        != columns.len()
    {
        return Err("Tick 欄位名稱重複".into());
    }
    let column = |name: &str| -> Result<usize, String> {
        let hits = columns
            .iter()
            .enumerate()
            .filter(|(_, s)| s.as_str() == name)
            .map(|(i, _)| i)
            .collect::<Vec<_>>();
        if hits.len() != 1 {
            Err(format!("需要唯一欄位：{name}"))
        } else {
            Ok(hits[0])
        }
    };
    let datetime = columns.iter().position(|s| s == "datetime");
    let date = if datetime.is_none() {
        Some(column("date")?)
    } else {
        None
    };
    let time = if datetime.is_none() {
        Some(column("time")?)
    } else {
        None
    };
    let price = column("price")?;
    let volume = column("volume")?;
    let mut ticks = 0u64;
    let mut bars = 0u64;
    let mut sum = 0i128;
    let mut outside_sum = 0i128;
    let mut outside_ticks = 0u64;
    let mut outside_writer: Option<csv::Writer<File>> = None;
    let mut previous_all = None;
    let mut first = String::new();
    let mut last = String::new();
    // Cache identical seconds, retaining every row and its input order.
    let mut cached_raw = String::new();
    let mut cached_dt = None;
    for record in reader.records() {
        let row = record.map_err(|e| e.to_string())?;
        let raw = if let Some(i) = datetime {
            row.get(i).ok_or("缺少時間欄位")?.to_string()
        } else {
            format!(
                "{} {}",
                row.get(date.unwrap()).ok_or("缺少日期")?,
                row.get(time.unwrap()).ok_or("缺少時間")?
            )
        };
        let dt = if raw == cached_raw {
            cached_dt.unwrap()
        } else {
            let mut parsed = None;
            for format in [
                "%Y/%m/%d %H:%M:%S%.f",
                "%Y-%m-%dT%H:%M:%S%.f",
                "%Y-%m-%d %H:%M:%S%.f",
            ] {
                if let Ok(dt) = NaiveDateTime::parse_from_str(&raw, format) {
                    parsed = Some(dt);
                    break;
                }
            }
            let dt = parsed.ok_or_else(|| format!("第 {} 筆時間無效：{raw}", ticks + 1))?;
            cached_raw = raw.clone();
            cached_dt = Some(dt);
            dt
        };
        let v = row.get(volume).ok_or("缺少成交量")?;
        if previous_all.is_some_and(|p| dt < p) {
            return Err(format!("第 {} 筆時間倒退", ticks + 1));
        }
        previous_all = Some(dt);
        if dt.nanosecond() >= 1_000_000_000 {
            return Err("不支援閏秒時間戳記".into());
        }
        let amount = decimal(v)?;
        if amount < 0 {
            return Err(format!("第 {} 筆成交量為負", ticks + 1));
        }
        let px = row.get(price).ok_or("缺少成交價")?;
        decimal(px)?;
        sum = sum.checked_add(decimal(v)?).ok_or("總成交量溢位")?;
        if aggregator.window(dt).is_none() {
            outside_sum = outside_sum.checked_add(amount).ok_or("盤外成交量溢位")?;
            outside_ticks += 1;
            if outside_writer.is_none() {
                let f = OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(output.join("outside_sessions.csv.partial"))
                    .map_err(|e| e.to_string())?;
                let mut w = csv::Writer::from_writer(f);
                w.write_record(["ordinal", "datetime", "price", "volume"])
                    .map_err(|e| e.to_string())?;
                outside_writer = Some(w);
            }
            outside_writer
                .as_mut()
                .unwrap()
                .write_record([&(ticks + 1).to_string(), &raw, px, v])
                .map_err(|e| e.to_string())?;
            ticks += 1;
            if first.is_empty() {
                first = raw.clone()
            }
            last = raw;
            continue;
        }
        if let Some(bar) = aggregator
            .push(dt, row.get(price).ok_or("缺少成交價")?, v)
            .map_err(|e| format!("第 {} 筆：{e}", ticks + 1))?
        {
            writer.serialize(bar).map_err(|e| e.to_string())?;
            bars += 1
        }
        ticks += 1;
        if first.is_empty() {
            first = raw.clone()
        }
        last = raw;
        if ticks % 10_000_000 == 0 {
            eprintln!("已處理 {ticks} 筆 Tick，產生 {bars} 根 {seconds} 秒 K")
        }
    }
    if ticks == 0 {
        return Err("沒有成交資料".into());
    }
    if let Some(bar) = aggregator.finish()? {
        writer.serialize(bar).map_err(|e| e.to_string())?;
        bars += 1
    }
    writer.flush().map_err(|e| e.to_string())?;
    drop(writer);
    if let Some(mut w) = outside_writer {
        w.flush().map_err(|e| e.to_string())?;
        drop(w);
        fs::rename(
            output.join("outside_sessions.csv.partial"),
            output.join("outside_sessions.csv"),
        )
        .map_err(|e| e.to_string())?;
    }
    let hash = reader.into_inner();
    let summary = Summary {
        source: input
            .canonicalize()
            .map_err(|e| e.to_string())?
            .display()
            .to_string(),
        source_bytes: bytes,
        source_sha256: format!("{:x}", hash.hash.finalize()),
        seconds,
        anchor_seconds: anchor,
        alignment: if sessions {
            "session_start"
        } else {
            "wall_clock"
        }
        .into(),
        sessions: if sessions {
            vec!["08:45:00–13:45:00".into(), "15:00:00–次日05:00:00".into()]
        } else {
            vec![]
        },
        timestamp_role: "end".into(),
        interval_rule: if sessions {
            "[start, end)；收盤瞬間的成交納入該交易時段末棒；標記結束時間"
        } else {
            "[start, end)；標記區間結束；按原始檔案順序取得開收盤"
        }
        .into(),
        empty_intervals: "不補空棒".into(),
        timezone: "unspecified".into(),
        input_ticks: ticks,
        aggregated_ticks: ticks - outside_ticks,
        outside_session_ticks: outside_ticks,
        output_bars: bars,
        total_volume: text(sum),
        aggregated_volume: text(sum - outside_sum),
        outside_session_volume: text(outside_sum),
        first_tick: first,
        last_tick: last,
        last_bar_may_be_partial: true,
    };
    fs::rename(&part, output.join("bars.csv")).map_err(|e| e.to_string())?;
    fs::write(
        output.join("aggregation.json"),
        serde_json::to_vec_pretty(&summary).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn dt(s: &str) -> NaiveDateTime {
        NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S%.f").unwrap()
    }
    #[test]
    fn day_session_anchors_at_0845() {
        let mut a = Aggregator::with_sessions(600).unwrap();
        a.push(dt("2026-10-23 08:45:00"), "100", "1").unwrap();
        let b = a
            .push(dt("2026-10-23 08:55:00"), "101", "1")
            .unwrap()
            .unwrap();
        assert_eq!(b.datetime, "2026-10-23T08:55:00");
        assert_eq!(a.finish().unwrap().unwrap().datetime, "2026-10-23T09:05:00");
    }
    #[test]
    fn closing_print_stays_in_last_session_bar() {
        let mut a = Aggregator::with_sessions(600).unwrap();
        a.push(dt("2026-10-23 13:44:59"), "100", "2").unwrap();
        assert!(a
            .push(dt("2026-10-23 13:45:00"), "101", "3")
            .unwrap()
            .is_none());
        let b = a
            .push(dt("2026-10-23 15:00:00"), "102", "1")
            .unwrap()
            .unwrap();
        assert_eq!(b.datetime, "2026-10-23T13:45:00");
        assert_eq!(b.volume, "5");
        assert_eq!(b.close, "101");
        assert!(a.window(dt("2026-10-23 14:00:00")).is_none());
        assert!(a.window(dt("2026-10-23 13:45:00.1")).is_none());
        assert_eq!(a.finish().unwrap().unwrap().datetime, "2026-10-23T15:10:00");
    }
    #[test]
    fn night_session_crosses_midnight_and_retains_close() {
        let mut a = Aggregator::with_sessions(600).unwrap();
        a.push(dt("2026-10-22 23:59:59"), "100", "1").unwrap();
        assert_eq!(
            a.push(dt("2026-10-23 00:00:00"), "101", "1")
                .unwrap()
                .unwrap()
                .datetime,
            "2026-10-23T00:00:00"
        );
        a.push(dt("2026-10-23 05:00:00"), "102", "1").unwrap();
        assert_eq!(a.finish().unwrap().unwrap().datetime, "2026-10-23T05:00:00");
    }
    #[test]
    fn duplicate_seconds_and_boundary() {
        let mut a = Aggregator::new(600, 0).unwrap();
        a.push(dt("2026-10-23 15:00:00"), "100.000000", "23")
            .unwrap();
        a.push(dt("2026-10-23 15:00:00"), "99.5", "1").unwrap();
        a.push(dt("2026-10-23 15:09:59.999999999"), "102", "2")
            .unwrap();
        let b = a
            .push(dt("2026-10-23 15:10:00"), "103", "4")
            .unwrap()
            .unwrap();
        assert_eq!(
            (&b.open, &b.high, &b.low, &b.close, &b.volume, b.tick_count),
            (
                &"100".into(),
                &"102".into(),
                &"99.5".into(),
                &"102".into(),
                &"26".into(),
                3
            )
        );
        assert_eq!(b.datetime, "2026-10-23T15:10:00");
        assert_eq!(a.finish().unwrap().unwrap().volume, "4");
    }
    #[test]
    fn gaps_do_not_create_bars_and_midnight_is_ordered() {
        let mut a = Aggregator::new(600, 0).unwrap();
        a.push(dt("2026-10-22 23:59:59"), "1", "0.1").unwrap();
        let b = a
            .push(dt("2026-10-23 00:20:00"), "2", "0.2")
            .unwrap()
            .unwrap();
        assert_eq!(b.datetime, "2026-10-23T00:00:00");
        assert_eq!(a.finish().unwrap().unwrap().tick_count, 1);
    }
    #[test]
    fn unsorted_and_invalid_input_rejected() {
        let mut a = Aggregator::new(600, 0).unwrap();
        a.push(dt("2026-10-23 15:00:01"), "1", "1").unwrap();
        assert!(a.push(dt("2026-10-23 15:00:00"), "1", "1").is_err());
        assert!(decimal("NaN").is_err());
        assert!(decimal("1.0000001").is_err());
        assert!(Aggregator::new(0, 0).is_err());
    }
    #[test]
    fn decimal_volume_is_exact() {
        assert_eq!(
            text(decimal("0.1").unwrap() + decimal("0.2").unwrap()),
            "0.3"
        );
        assert!(decimal("--1").is_err());
        assert!(decimal("+-1").is_err());
    }
    #[test]
    fn close_exclusive_300s_preserves_anchors_and_excludes_both_closes() {
        let a=Aggregator::with_sessions_close_exclusive(300).unwrap();
        for close in ["2026-10-23 13:45:00", "2026-10-24 05:00:00"] {assert!(a.window(dt(close)).is_none());}
        let (start,end)=a.window(dt("2026-10-23 08:45:00")).unwrap();assert_eq!(end-start,300);
        assert_eq!(end,dt("2026-10-23 08:50:00").and_utc().timestamp());
        let (_,end)=a.window(dt("2026-10-23 08:50:00")).unwrap();assert_eq!(end,dt("2026-10-23 08:55:00").and_utc().timestamp());
        let (_,end)=a.window(dt("2026-10-24 04:59:59")).unwrap();assert_eq!(end,dt("2026-10-24 05:00:00").and_utc().timestamp());
    }
}
