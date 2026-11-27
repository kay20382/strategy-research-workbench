use crate::{live_market, live_strategy, tick_bars, trading, workbench};
use chrono::{DateTime, Days, NaiveDate, NaiveDateTime, NaiveTime, TimeDelta};
use godot::prelude::*;
use rusqlite::{params, Connection, OpenFlags, OptionalExtension};
use serde_json::{json, Value};
use std::{collections::BTreeMap, path::Path, sync::mpsc, time::Duration};

fn text(value: &Value) -> &str {
    value.as_str().unwrap_or_default().trim()
}

fn read_db(path: &Path) -> Result<Connection, String> {
    let db = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|e| e.to_string())?;
    db.busy_timeout(Duration::from_millis(500))
        .map_err(|e| e.to_string())?;
    Ok(db)
}

fn table(db: &Connection, name: &str) -> rusqlite::Result<bool> {
    db.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1)",
        [name],
        |r| r.get(0),
    )
}

fn number(value: &Value) -> Option<f64> {
    let n = value.as_f64().or_else(|| text(value).parse().ok())?;
    n.is_finite().then_some(n)
}

fn broker_time(fill: &Value) -> Option<NaiveDateTime> {
    // ReportTime2 may carry a full calendar timestamp. Only accept a complete date and time.
    let full = text(&fill["report_time2"]);
    if let Ok(timestamp) = live_market::time(full) {
        return Some(timestamp);
    }
    if full.len() >= 14 && full.bytes().all(|c| c.is_ascii_digit()) {
        let joined = format!(
            "{}{}{}",
            &full[..14],
            if full.len() > 14 { "." } else { "" },
            &full[14..]
        );
        if let Ok(timestamp) = NaiveDateTime::parse_from_str(&joined, "%Y%m%d%H%M%S%.f") {
            return Some(timestamp);
        }
    }
    let raw = text(&fill["time"]);
    let clock = if raw.contains(':') {
        NaiveTime::parse_from_str(raw, "%H:%M:%S%.f").ok()?
    } else if (6..=12).contains(&raw.len()) && raw.bytes().all(|c| c.is_ascii_digit()) {
        let joined = format!(
            "{}{}{}",
            &raw[..6],
            if raw.len() > 6 { "." } else { "" },
            &raw[6..]
        );
        NaiveTime::parse_from_str(&joined, "%H%M%S%.f").ok()?
    } else {
        return None;
    };
    if text(&fill["session"]) == "R" {
        let date = text(&fill["trade_date"]);
        for format in ["%Y%m%d", "%Y-%m-%d", "%Y/%m/%d"] {
            if let Ok(date) = NaiveDate::parse_from_str(date, format) {
                return Some(date.and_time(clock));
            }
        }
    }
    // Night reports can belong to the next trading day. For a newly received report,
    // resolve the calendar date from the Taipei receive clock only within a tight window.
    if text(&fill["session"]) != "P" || fill["recovered"] != false {
        return None;
    }
    let received = live_market::time(text(&fill["received_at"])).ok()?;
    let today = received.date();
    [
        today.checked_sub_days(Days::new(1)),
        Some(today),
        today.checked_add_days(Days::new(1)),
    ]
    .into_iter()
    .flatten()
    .map(|day| day.and_time(clock))
    .find(|time| (-2000..=120_000).contains(&(received - *time).num_milliseconds()))
}

fn monthly_catalog_alias(symbol: &str) -> Option<(&'static str, &'static str)> {
    match symbol.as_bytes() {
        [b'T', b'X', b'F', b'A'..=b'L', b'0'..=b'9'] => Some(("TXF1", "台指期近月")),
        [b'M', b'X', b'F', b'A'..=b'L', b'0'..=b'9'] => Some(("MXF", "小型臺指期貨")),
        [b'T', b'M', b'F', b'A'..=b'L', b'0'..=b'9'] => Some(("TMF", "微型臺指期貨")),
        [b'C', b'D', b'F', b'A'..=b'L', b'0'..=b'9'] => Some(("CDF", "台積電期貨")),
        _ => {
            let maturity = symbol.strip_prefix("225M-")?;
            if maturity.len() == 6 && maturity.bytes().all(|byte| byte.is_ascii_digit())
                && (1..=12).contains(&maturity[4..].parse::<u8>().ok()?)
            {
                Some(("225M", "小型日經225期貨"))
            } else {
                None
            }
        }
    }
}

fn catalog(db: &Connection, products: &mut BTreeMap<String, String>) -> rusqlite::Result<()> {
    for sql in [
        (
            "market_sources",
            "SELECT DISTINCT symbol,'' FROM market_sources",
        ),
        ("live_sources", "SELECT symbol,'' FROM live_sources"),
        (
            "qm_instruments",
            "SELECT symbol,COALESCE(json_extract(config,'$.name'),'') FROM qm_instruments",
        ),
    ] {
        if !table(db, sql.0)? {
            continue;
        }
        let mut query = db.prepare(sql.1)?;
        let rows = query.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
        for row in rows {
            let (key, name) = row?;
            let key = key.to_ascii_uppercase();
            if let Some((product, fallback_name)) = monthly_catalog_alias(&key) {
                let existing = products.entry(product.into()).or_default();
                if existing.is_empty() {
                    *existing = fallback_name.into();
                }
            } else {
                let existing = products.entry(key).or_default();
                if !name.is_empty() {
                    *existing = name;
                }
            }
        }
    }
    Ok(())
}

const CHART_LIMIT: usize = 720;
const RANGE_LIMIT: usize = 5_000;
const RANGE_TICK_READ_LIMIT: usize = 300_000;

#[derive(Clone, Copy, Default)]
struct DateRange {
    from: Option<NaiveDate>,
    to: Option<NaiveDate>,
}

impl DateRange {
    fn parse(from: &str, to: &str) -> Result<Self, String> {
        let parse = |date: &str| {
            if date.is_empty() {
                Ok(None)
            } else {
                NaiveDate::parse_from_str(date, "%Y-%m-%d")
                    .map(Some)
                    .map_err(|_| "日期格式無效，請使用 YYYY/MM/DD".to_owned())
            }
        };
        let (from, to) = (parse(from)?, parse(to)?);
        if from.zip(to).is_some_and(|(from, to)| from > to) {
            return Err("起日不得晚於迄日".into());
        }
        Ok(Self { from, to })
    }

    fn active(self) -> bool {
        self.from.is_some() || self.to.is_some()
    }

    fn record_bounds(self, period: Period) -> (String, String) {
        // Include the source rows just outside the dates if they contribute
        // to an aggregate whose label is inside the selected range.
        let (before, after) = match period {
            Period::Minute(n) if n > 1 => (1, 0),
            Period::Daily(n) => (n - 1, n - 1),
            _ => (0, 0),
        };
        let from = self
            .from
            .and_then(|d| d.checked_sub_days(Days::new(before as u64)));
        let to = self
            .to
            .and_then(|d| d.checked_add_days(Days::new(after as u64 + 1)));
        (
            from.map(|d| d.to_string()).unwrap_or_default(),
            to.map(|d| d.to_string()).unwrap_or_else(|| "~".into()),
        )
    }

    fn tick_bounds(self, period: Period) -> (i64, i64) {
        let padding = if period == Period::Tick {
            0
        } else {
            i128::from(period.amount()) * 1_000_000_000
        };
        let nanos = |d: NaiveDate| {
            i128::from(d.and_time(NaiveTime::MIN).and_utc().timestamp()) * 1_000_000_000
        };
        let clamp = |ns: i128| ns.clamp(i128::from(i64::MIN), i128::from(i64::MAX)) as i64;
        (
            self.from
                .map(|d| clamp(nanos(d) - padding))
                .unwrap_or(i64::MIN),
            self.to
                .and_then(|d| d.succ_opt())
                .map(|d| clamp(nanos(d) - 1))
                .unwrap_or(i64::MAX),
        )
    }

    fn contains(self, stamp: &str) -> bool {
        !self.active()
            || stamp
                .get(..10)
                .and_then(|stamp| NaiveDate::parse_from_str(stamp, "%Y-%m-%d").ok())
                .is_some_and(|d| {
                    self.from.is_none_or(|from| d >= from) && self.to.is_none_or(|to| d <= to)
                })
    }

    fn limit(self) -> usize {
        if self.active() {
            RANGE_LIMIT
        } else {
            CHART_LIMIT
        }
    }

    fn trim(self, bars: &mut Vec<Value>) -> bool {
        bars.retain(|bar| self.contains(text(&bar[0])));
        let extra = if self.active() {
            0
        } else {
            bars.len().saturating_sub(CHART_LIMIT)
        };
        bars.drain(..extra);
        extra > 0
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Period {
    Auto,
    Tick,
    Second(i64),
    Minute(i64),
    Daily(i64),
}

impl Period {
    fn parse(kind: &str, amount: i64) -> Result<Self, String> {
        match kind {
            "auto" => Ok(Self::Auto),
            "tick" => Ok(Self::Tick),
            "second" if (1..=60).contains(&amount) => Ok(Self::Second(amount)),
            "minute" if (1..=240).contains(&amount) => Ok(Self::Minute(amount)),
            "daily" if (1..=30).contains(&amount) => Ok(Self::Daily(amount)),
            _ => Err("不支援的圖表週期".into()),
        }
    }

    fn amount(self) -> i64 {
        match self {
            Self::Minute(n) | Self::Daily(n) | Self::Second(n) => n,
            _ => 1,
        }
    }
}

fn windows(seconds: i64, policy: &str) -> Result<tick_bars::Aggregator, String> {
    if policy == "taifex_regular" {
        tick_bars::Aggregator::with_sessions(seconds)
    } else if policy == live_market::STOCK_POLICY {
        tick_bars::Aggregator::with_stock_sessions(seconds)
    } else {
        tick_bars::Aggregator::new(seconds, 0)
    }
}

fn end_stamp(seconds: i64) -> Option<String> {
    DateTime::from_timestamp(seconds, 0).map(|dt| live_market::stamp(dt.naive_utc()))
}

struct Candle {
    datetime: String,
    prices: [f64; 5],
    pending: bool,
}

impl Candle {
    fn merge(&mut self, bar: Self) {
        let [_, high, low, close, volume] = bar.prices;
        self.prices[1] = self.prices[1].max(high);
        self.prices[2] = self.prices[2].min(low);
        self.prices[3] = close;
        self.prices[4] += volume;
        self.datetime = bar.datetime;
        self.pending |= bar.pending;
    }

    fn encode(self) -> Value {
        let [o, h, l, c, v] = self.prices;
        json!([self.datetime, o, h, l, c, v, self.pending])
    }
}

struct BarSeries {
    period: Period,
    starts: bool,
    windows: tick_bars::Aggregator,
    key: Option<i64>,
    bars: Vec<Candle>,
}

impl BarSeries {
    fn new(period: Period, role: &str, policy: &str) -> Result<Self, String> {
        Ok(Self {
            period,
            starts: role == "start",
            windows: windows(period.amount() * 60, policy)?,
            key: None,
            bars: Vec::new(),
        })
    }

    fn push(&mut self, mut bar: Candle) -> Result<(), String> {
        let amount = self.period.amount();
        if amount == 1 {
            self.bars.push(bar);
            return Ok(());
        }
        let key = if matches!(self.period, Period::Daily(_)) {
            let date = NaiveDate::parse_from_str(
                bar.datetime.get(..10).unwrap_or(&bar.datetime),
                "%Y-%m-%d",
            )
            .map_err(|e| e.to_string())?;
            let day = date
                .and_time(NaiveTime::MIN)
                .and_utc()
                .timestamp()
                .div_euclid(86400);
            bar.datetime = date.to_string();
            day.div_euclid(amount)
        } else {
            let at = live_market::time(&bar.datetime)?;
            let sample = if self.starts {
                at
            } else {
                at - TimeDelta::nanoseconds(1)
            };
            let Some((_, end)) = self.windows.window(sample) else {
                return Ok(());
            };
            if self.key != Some(end) {
                bar.datetime = end_stamp(end).ok_or("K 棒時間超出範圍")?;
            } else if let Some(active) = self.bars.last() {
                bar.datetime.clone_from(&active.datetime);
            }
            end
        };
        if self.key == Some(key) {
            self.bars
                .last_mut()
                .expect("a candle exists for the current bucket")
                .merge(bar);
        } else {
            self.key = Some(key);
            self.bars.push(bar);
        }
        Ok(())
    }

    fn finish(
        mut self,
        range: DateRange,
        closed_through: Option<NaiveDateTime>,
    ) -> Result<(Vec<Value>, usize), String> {
        if self.period.amount() > 1 {
            if let (Some(bar), Some(closed)) = (self.bars.last_mut(), closed_through) {
                bar.pending = live_market::time(&bar.datetime)? > closed;
            }
        }
        self.bars.retain(|bar| range.contains(&bar.datetime));
        if !range.active() {
            let extra = self.bars.len().saturating_sub(CHART_LIMIT);
            self.bars.drain(..extra);
        }
        // Keep exact candles for normal ranges. Very dense multi-year ranges
        // use an explicitly labelled full-span overview instead of dropping old rows.
        let group = self.bars.len().div_ceil(300_000).max(1);
        let mut output = Vec::with_capacity(self.bars.len().div_ceil(group));
        let mut input = self.bars.into_iter();
        while let Some(mut bar) = input.next() {
            let first = (group > 1).then(|| bar.datetime.clone());
            for extra in input.by_ref().take(group - 1) {
                bar.merge(extra);
            }
            let mut encoded = bar.encode();
            if let Some(start) = first {
                encoded
                    .as_array_mut()
                    .expect("candle encoding is an array")
                    .push(json!(start));
            }
            output.push(encoded);
        }
        Ok((output, group))
    }
}

fn read_closed(
    db: &Connection,
    dataset: i64,
    period: Period,
    range: DateRange,
    series: &mut BarSeries,
) -> Result<Option<NaiveDateTime>, String> {
    let (from, to) = range.record_bounds(period);
    let limit = (CHART_LIMIT * period.amount() as usize).min(12_000) as i64;
    let sql = if range.active() {
        "SELECT datetime,payload FROM records WHERE dataset_id=?1 AND datetime>=?2 AND datetime<?3 ORDER BY datetime,ordinal"
    } else {
        "SELECT datetime,payload FROM (SELECT datetime,payload,ordinal FROM records WHERE dataset_id=?1 AND datetime>=?2 AND datetime<?3 ORDER BY datetime DESC,ordinal DESC LIMIT ?4) ORDER BY datetime,ordinal"
    };
    let mut query = db.prepare(sql).map_err(|e| e.to_string())?;
    let mut rows = if range.active() {
        query.query(params![dataset, from, to])
    } else {
        query.query(params![dataset, from, to, limit])
    }
    .map_err(|e| e.to_string())?;
    let mut last = String::new();
    while let Some(row) = rows.next().map_err(|e| e.to_string())? {
        let (time, raw): (String, String) = (
            row.get(0).map_err(|e| e.to_string())?,
            row.get(1).map_err(|e| e.to_string())?,
        );
        let Some(prices) = serde_json::from_str::<Value>(&raw).ok().and_then(|bar| {
            Some([
                number(&bar["open"])?,
                number(&bar["high"])?,
                number(&bar["low"])?,
                number(&bar["close"])?,
                number(&bar["volume"])?,
            ])
        }) else {
            continue;
        };
        last.clone_from(&time);
        series.push(Candle {
            datetime: time,
            prices,
            pending: false,
        })?;
    }
    Ok(live_market::time(&last).ok())
}

fn historical(
    db: &Connection,
    symbol: &str,
    period: Period,
    range: DateRange,
) -> Result<Value, String> {
    if !table(db, "market_sources").map_err(|e| e.to_string())? {
        return Ok(Value::Null);
    }
    let kind_filter = match period {
        Period::Minute(_) => "minute",
        Period::Daily(_) => "daily",
        _ => "",
    };
    if matches!(period, Period::Auto | Period::Minute(_) | Period::Daily(_)) {
        let (from, to) = range.record_bounds(period);
        let source=db.query_row("SELECT m.dataset_id,m.kind,d.timezone,d.timestamp_role FROM market_sources m
            JOIN datasets d ON d.id=m.dataset_id WHERE m.symbol=?1 AND m.kind IN ('minute','daily') AND d.row_count>0
            AND (?2='' OR m.kind=?2) AND EXISTS(SELECT 1 FROM records r WHERE r.dataset_id=d.id AND r.datetime>=?3 AND r.datetime<?4)
            ORDER BY CASE m.kind WHEN 'minute' THEN 0 ELSE 1 END LIMIT 1",
            params![symbol,kind_filter,from,to],|r|{
                Ok((r.get::<_,i64>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,String>(3)?))
            }).optional().map_err(|e| e.to_string())?;
        if let Some((id, kind, zone, role)) = source {
            let amount = period.amount();
            let mut series = BarSeries::new(period, &role, "")?;
            read_closed(db, id, period, range, &mut series)?;
            let (bars, group) = series.finish(range, None)?;
            return Ok(
                json!({"bars":bars,"kind":kind,"amount":amount,"zone":zone,"role":role,"source":"history","broker_symbol":"","overview_group":group}),
            );
        }
        if period != Period::Auto {
            return Ok(Value::Null);
        }
    }
    let tick=db.query_row("SELECT m.dataset_id,d.last_datetime,d.timezone FROM market_sources m JOIN datasets d ON d.id=m.dataset_id
      WHERE m.kind='tick' AND m.symbol=?1 AND d.row_count>0 AND d.last_datetime IS NOT NULL",[symbol],|r| {
        Ok((r.get::<_,i64>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?))
    }).optional().map_err(|e|e.to_string())?;
    let Some((id, last, zone)) = tick else {
        return Ok(Value::Null);
    };
    let (bars, kind, amount, overview) = tick_chart(db, id, &last, period, "", None, range)?;
    Ok(
        json!({"bars":bars,"kind":kind,"amount":amount,"zone":zone,"source":"history","broker_symbol":"","tick_overview":overview}),
    )
}

fn tick_chart(
    db: &Connection,
    id: i64,
    last: &str,
    period: Period,
    policy: &str,
    watermark: Option<NaiveDateTime>,
    range: DateRange,
) -> Result<(Vec<Value>, &'static str, i64, bool), String> {
    if !crate::tick_store::contains(db, id)? {
        return Ok((Vec::new(), "tick", 1, false));
    }
    if period == Period::Tick {
        let ticks =
            crate::tick_store::recent(db, id, range.limit() + 1, range.tick_bounds(period))?;
        if range.active() && ticks.len() > range.limit() {
            return tick_overview(db, id, last, period, policy, range);
        }
        let mut bars = ticks
            .into_iter()
            .map(|[ns, price, qty]| {
                let time = live_market::stamp(DateTime::from_timestamp_nanos(ns).naive_utc());
                let price = tick_bars::text(price.into());
                json!([
                    time,
                    price,
                    price,
                    price,
                    price,
                    tick_bars::text(qty.into()),
                    false
                ])
            })
            .collect::<Vec<_>>();
        range.trim(&mut bars);
        return Ok((bars, "tick", 1, false));
    }
    let seconds = match period {
        Period::Second(n) => n,
        _ => 1,
    };
    let last = live_market::time(last)?;
    let mut agg = windows(seconds, policy)?;
    let mut bars = Vec::new();
    let encode = |bar: tick_bars::TickBar, forming| {
        json!([
            bar.datetime,
            bar.open,
            bar.high,
            bar.low,
            bar.close,
            bar.volume,
            forming
        ])
    };
    let mut consume = |ns, price: i64, qty: i64| {
        if let Some(bar) = agg.push_fixed(
            DateTime::from_timestamp_nanos(ns).naive_utc(),
            price.into(),
            qty.into(),
        )? {
            bars.push(encode(bar, false));
        }
        Ok(())
    };
    if range.active() {
        let ticks = crate::tick_store::recent(
            db,
            id,
            RANGE_TICK_READ_LIMIT + 1,
            range.tick_bounds(period),
        )?;
        if ticks.len() > RANGE_TICK_READ_LIMIT {
            return tick_overview(db, id, &live_market::stamp(last), period, policy, range);
        }
        for [ns, price, qty] in ticks {
            consume(ns, price, qty)?;
        }
    } else {
        let Some((start, _)) = windows(seconds, policy)?.window(last) else {
            return Ok((Vec::new(), "second", seconds, false));
        };
        let first =
            end_stamp(start - seconds * (CHART_LIMIT as i64 - 1)).ok_or("Tick 時間超出範圍")?;
        crate::tick_store::visit(db, id, &first, &live_market::stamp(last), consume)?;
    }
    if let Some(bar) = agg.finish()? {
        let forming = !policy.is_empty()
            && live_market::time(&bar.datetime)? > watermark.map_or(last, |at| at.max(last));
        bars.push(encode(bar, forming));
    }
    range.trim(&mut bars);
    Ok((bars, "second", seconds, false))
}

fn tick_overview(
    db: &Connection,
    id: i64,
    last: &str,
    period: Period,
    policy: &str,
    range: DateRange,
) -> Result<(Vec<Value>, &'static str, i64, bool), String> {
    let first: String = db
        .query_row(
            "SELECT first_datetime FROM datasets WHERE id=?1",
            [id],
            |row| row.get(0),
        )
        .map_err(|e| e.to_string())?;
    let nanos = |value: &str| {
        live_market::time(value)?
            .and_utc()
            .timestamp_nanos_opt()
            .ok_or("Tick 時間超出範圍".to_owned())
    };
    let (lo, hi) = range.tick_bounds(period);
    let from = lo.max(nanos(&first)?);
    let to = hi.min(nanos(last)?);
    let span = (i128::from(to) - i128::from(from)).max(0) / 1_000_000_000;
    let unit = period.amount();
    let seconds = ((span / (RANGE_LIMIT as i128 * i128::from(unit)) + 1) * i128::from(unit))
        .clamp(i128::from(unit), 86_400) as i64;
    let within = |ns| live_market::stamp(DateTime::from_timestamp_nanos(ns).naive_utc());
    let mut agg = windows(seconds, policy)?;
    let mut bars = Vec::new();
    let encode = |b: tick_bars::TickBar| {
        json!([b.datetime, b.open, b.high, b.low, b.close, b.volume, false])
    };
    crate::tick_store::visit(db, id, &within(from), &within(to), |ns, price, qty| {
        if let Some(bar) = agg.push_fixed(
            DateTime::from_timestamp_nanos(ns).naive_utc(),
            price.into(),
            qty.into(),
        )? {
            bars.push(encode(bar));
        }
        Ok(())
    })?;
    if let Some(bar) = agg.finish()? {
        bars.push(encode(bar));
    }
    // Anchor overview bars to the selected calendar bounds for display.
    if let (Some(end), Some(bar)) = (range.to, bars.last_mut()) {
        if !range.contains(text(&bar[0])) {
            let last = end
                .and_hms_nano_opt(23, 59, 59, 999_999_999)
                .ok_or("迄日超出範圍")?;
            bar[0] = json!(live_market::stamp(last));
        }
    }
    range.trim(&mut bars);
    Ok((bars, "second", seconds, true))
}

fn snapshot(
    db: &Connection,
    symbol: &str,
    period: Period,
    range: DateRange,
) -> Result<Value, String> {
    if matches!(period, Period::Daily(_))
        || !table(db, "live_sources").map_err(|e| e.to_string())?
    {
        return Ok(Value::Null);
    }
    let source = db
        .query_row(
            "SELECT s.minute_id,s.tick_id,s.session_policy,s.timezone,d.last_datetime,w.cutoff
             FROM live_sources s JOIN datasets d ON d.id=s.tick_id
             LEFT JOIN live_watermarks w ON w.provider=s.provider AND w.connection=s.connection
             AND w.timezone=s.timezone AND w.session_policy=s.session_policy WHERE s.symbol=?1",
            [symbol],
            |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, Option<String>>(4)?,
                    r.get::<_, Option<String>>(5)?,
                ))
            },
        )
        .optional()
        .map_err(|e| e.to_string())?;
    let Some((dataset, tick_id, policy, timezone, last_tick, cutoff)) = source else {
        return Ok(Value::Null);
    };
    let watermark = cutoff.as_deref().map(live_market::time).transpose()?;
    let broker_symbol = broker_symbol(db, symbol).map_err(|e| e.to_string())?;
    if matches!(period, Period::Tick | Period::Second(_)) {
        let Some(last_tick) = last_tick else {
            return Ok(Value::Null);
        };
        let (bars, kind, amount, overview) =
            tick_chart(db, tick_id, &last_tick, period, &policy, watermark, range)?;
        return Ok(
            json!({"bars":bars,"kind":kind,"amount":amount,"source":"live","policy":policy,"zone":timezone,"broker_symbol":broker_symbol,"tick_overview":overview}),
        );
    }
    let amount = period.amount();
    let mut series = BarSeries::new(period, "end", &policy)?;
    let closed = read_closed(db, dataset, period, range, &mut series)?;
    let latest = closed
        .into_iter()
        .chain(watermark)
        .chain(last_tick.as_deref().map(live_market::time).transpose()?)
        .max();
    for bar in pending_minutes(db, symbol).map_err(|e| e.to_string())? {
        if range.contains(&bar.datetime) {
            series.push(bar)?;
        }
    }
    let (bars, group) = series.finish(range, Some(latest.unwrap_or(NaiveDateTime::MIN)))?;
    Ok(
        json!({"bars":bars,"kind":"minute","amount":amount,"source":"live","policy":policy,"zone":timezone,"broker_symbol":broker_symbol,"overview_group":group}),
    )
}

fn pending_minutes(db: &Connection, symbol: &str) -> rusqlite::Result<Vec<Candle>> {
    let mut query = db.prepare(
        "SELECT bar_end,price,volume FROM live_pending WHERE symbol=?1 ORDER BY bar_end,event_id",
    )?;
    let mut rows = query.query([symbol])?;
    let mut pending = BTreeMap::<String, [i128; 5]>::new();
    while let Some(r) = rows.next()? {
        let (end, price, volume) = (
            r.get::<_, String>(0)?,
            r.get::<_, i64>(1)? as i128,
            r.get::<_, i64>(2)? as i128,
        );
        let bar = pending
            .entry(end)
            .or_insert([price, price, price, price, 0]);
        bar[1] = bar[1].max(price);
        bar[2] = bar[2].min(price);
        bar[3] = price;
        bar[4] += volume;
    }
    Ok(pending
        .into_iter()
        .map(|(end, values)| {
            let prices = values.map(|x| tick_bars::text(x).parse::<f64>().unwrap_or_default());
            Candle {
                datetime: end,
                prices,
                pending: true,
            }
        })
        .collect())
}

fn broker_symbol(db: &Connection, symbol: &str) -> rusqlite::Result<String> {
    let pending_symbol: Option<String>=db.query_row(
        "SELECT json_extract(e.body,'$.provider_symbol') FROM live_pending p JOIN live_events e ON e.id=p.event_id WHERE p.symbol=?1 ORDER BY p.bar_end DESC,p.event_id DESC LIMIT 1", [symbol], |r| r.get(0)
    ).optional()?.flatten();
    if let Some(symbol) = pending_symbol {
        return Ok(symbol);
    }
    Ok(db.query_row("SELECT json_extract(body,'$.provider_symbol') FROM (SELECT body FROM live_events ORDER BY id DESC LIMIT 512) WHERE json_extract(body,'$.instrument')=?1 AND json_extract(body,'$.provider_symbol') IS NOT NULL LIMIT 1", [symbol], |r| r.get(0)).optional()?.unwrap_or_default())
}

fn annotations(root: &Path, data: &Value, profile: &str, contract: &str) -> Result<Value, String> {
    let path = trading::journal_path(root);
    if profile.is_empty() || contract.is_empty() || !path.is_file() || data["zone"] != "Asia/Taipei"
    {
        return Ok(json!({"fills":{},"unmapped":0}));
    }
    let db = read_db(&path)?;
    let continuous_product = text(&data["continuous_product"]);
    let expiries = (!continuous_product.is_empty()).then(|| live_strategy::expiry_overrides(root)).transpose()?;
    let build = || -> rusqlite::Result<Vec<String>> {
        if !table(&db, "trade_events")? {
            return Ok(vec![]);
        }
        let mut query=db.prepare("SELECT event FROM trade_events WHERE profile=?1 COLLATE NOCASE AND mode IN ('live','read_only') AND json_extract(event,'$.type')='fill' AND trim(json_extract(event,'$.data.symbol'))=?2 COLLATE NOCASE ORDER BY seq DESC LIMIT 2000")?;
        let rows = query.query_map(params![profile, contract], |r| r.get(0))?;
        rows.collect()
    };
    let tick_view = data["kind"] == "tick";
    let scale = if data["kind"] == "second" { 1 } else { 60 };
    let windows = windows(
        scale * data["amount"].as_i64().unwrap_or(1),
        text(&data["policy"]),
    )?;
    let mut indices = BTreeMap::<NaiveDateTime, Vec<(usize, Option<f64>)>>::new();
    for (index, bar) in data["bars"].as_array().into_iter().flatten().enumerate() {
        if let Ok(at) = live_market::time(text(&bar[0])) {
            indices
                .entry(at)
                .or_default()
                .push((index, number(&bar[4])));
        }
    }
    let mut seen = std::collections::HashSet::new();
    let mut grouped = BTreeMap::<usize, Vec<Value>>::new();
    let mut unmapped = 0;
    for raw in build().map_err(|e| e.to_string())? {
        let Ok(event) = serde_json::from_str::<Value>(&raw) else {
            continue;
        };
        let fill = &event["data"];
        let key = ["trade_date", "web_id", "cnt", "execution_id"]
            .map(|name| text(&fill[name]).to_owned());
        if key.iter().any(|part| part.is_empty()) || seen.contains(&key) {
            continue;
        }
        let side = match text(&fill["side"]) {
            "B" => 1,
            "S" => -1,
            _ => {
                unmapped += 1;
                continue;
            }
        };
        let Some(at) = broker_time(fill) else {
            unmapped += 1;
            continue;
        };
        if expiries.as_ref().is_some_and(|dates| {
            live_strategy::monthly_quote_contract(continuous_product, at, dates)
                .is_none_or(|(_, actual, _)| !actual.eq_ignore_ascii_case(contract))
        }) { continue; }
        let Some((price, qty)) = number(&fill["price"]).zip(number(&fill["quantity"])) else {
            continue;
        };
        if qty <= 0.0 {
            continue;
        }
        let index = if tick_view {
            let mut matches = indices
                .get(&at)
                .into_iter()
                .flatten()
                .filter(|(_, tick_price)| *tick_price == Some(price));
            let Some((first, _)) = matches.next() else {
                continue;
            };
            if matches.next().is_some() {
                continue;
            }
            *first
        } else {
            let Some(end) = windows
                .window(at)
                .and_then(|(_, end)| DateTime::from_timestamp(end, 0))
            else {
                unmapped += 1;
                continue;
            };
            let Some(bars) = indices.get(&end.naive_utc()) else {
                continue;
            };
            bars[0].0
        };
        grouped.entry(index).or_default().push(json!({"side":side,"price":price,"quantity":qty,
            "datetime":live_market::stamp(at),"order_no":fill["order_no"],"execution_id":fill["execution_id"]}));
        seen.insert(key);
    }
    Ok(json!({"fills":grouped,"unmapped":unmapped}))
}

fn continuous_near(
    root: &Path, path: &Path, history_path: &Path, product: &str, period: Period, range: DateRange,
) -> Result<Value, String> {
    if !path.is_file() { return Ok(Value::Null); }
    let family = if product == "TXF1" { "TXF" } else { product };
    let jpx = product == "225M";
    let (zone, policy, contract_pattern) = match product {
        "225M" => ("Asia/Tokyo", "clock_minute", "225M-[0-9][0-9][0-9][0-9][0-9][0-9]".into()),
        "CDF" => ("Asia/Taipei", live_market::STOCK_POLICY, format!("{family}[A-L][0-9]")),
        _ => ("Asia/Taipei", "taifex_regular", format!("{family}[A-L][0-9]")),
    };
    let expiries = (!jpx).then(|| live_strategy::expiry_overrides(root)).transpose()?;
    let db = read_db(path)?;
    let tx = db.unchecked_transaction().map_err(|e| e.to_string())?;
    if !table(&tx, "live_sources").map_err(|e| e.to_string())? { return Ok(Value::Null); }
    let mut query = tx.prepare("SELECT s.symbol,s.minute_id,d.last_datetime,w.cutoff,d.first_datetime
        FROM live_sources s JOIN datasets d ON d.id=s.tick_id
        LEFT JOIN live_watermarks w ON w.provider=s.provider AND w.connection=s.connection
           AND w.timezone=s.timezone AND w.session_policy=s.session_policy
        WHERE s.provider='kgi_quotecom' AND s.timezone=?1 AND s.session_policy=?2
          AND s.symbol GLOB ?3 AND d.row_count>0 ORDER BY s.symbol").map_err(|e| e.to_string())?;
    let contracts = query.query_map(params![zone, policy, contract_pattern], |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?,
        row.get::<_, Option<String>>(2)?, row.get::<_, Option<String>>(3)?,
        row.get::<_, Option<String>>(4)?))).map_err(|e| e.to_string())?
        .collect::<rusqlite::Result<Vec<_>>>().map_err(|e| e.to_string())?;
    let mut incoming = BTreeMap::new();
    let mut closed_through = None;
    let mut last_contract = String::new();
    let mut last_label = String::new();
    let mut last_seen = None;
    let (from, to) = range.record_bounds(period);
    for (index, (symbol, dataset, tick, cutoff, _)) in contracts.iter().enumerate() {
        // Osaka subscriptions select an explicit maturity. Once a later maturity starts
        // contributing, keep its candles and omit overlapping candles of older contracts.
        let next_jpx_start = if jpx {
            contracts[index + 1..].iter().filter_map(|(_, _, _, _, first)| first.as_deref())
                .min().map(live_market::time).transpose()?
        } else {
            None
        };
        let mut source = BarSeries::new(Period::Minute(1), "end", policy)?;
        let minute = read_closed(&tx, *dataset, period, range, &mut source)?;
        source.bars.extend(pending_minutes(&tx, symbol).map_err(|e| e.to_string())?);
        for mut bar in source.bars {
            if bar.datetime < from || bar.datetime >= to { continue; }
            let at = live_market::time(&bar.datetime)?;
            if let Some(dates) = &expiries {
                let instant = at - TimeDelta::nanoseconds(1);
                let Some((_, expected, paused)) = live_strategy::monthly_quote_contract(family, instant, dates) else { continue; };
                if paused || expected != *symbol { continue; }
            } else if next_jpx_start.is_some_and(|start| at > start) {
                continue;
            }
            // Preserve interval padding until the final aggregation and date filter.
            bar.datetime = live_market::stamp(at);
            if bar.datetime > last_label {
                last_label.clone_from(&bar.datetime);
                last_contract.clone_from(symbol);
                closed_through = minute.into_iter().chain(cutoff.as_deref().map(live_market::time).transpose()?)
                    .chain(tick.as_deref().map(live_market::time).transpose()?).max();
                last_seen = tick.as_deref().map(live_market::time).transpose()?;
            }
            incoming.insert(bar.datetime.clone(), bar);
        }
    }
    if !incoming.values().any(|bar| range.contains(&bar.datetime)) {
        return Ok(Value::Null);
    }

    let mut history = BarSeries::new(Period::Minute(1), "end", policy)?;
    let mut history_compatible = false;
    let history_result = (|| -> Result<(), String> {
        if history_path.is_file() {
            let historical_db = read_db(history_path)?;
            if table(&historical_db, "market_sources").map_err(|e| e.to_string())? {
                let dataset: Option<i64> = historical_db.query_row("SELECT m.dataset_id FROM market_sources m JOIN datasets d ON d.id=m.dataset_id
                    WHERE m.symbol=?1 COLLATE NOCASE AND m.kind='minute' AND d.timezone=?2 AND d.timestamp_role='end'", params![product, zone], |row| row.get(0))
                    .optional().map_err(|e| e.to_string())?;
                if let Some(dataset) = dataset {
                    read_closed(&historical_db, dataset, period, range, &mut history)?;
                    history_compatible = true;
                }
            }
        }
        Ok(())
    })();
    if history_result.is_err() { history.bars.clear(); }
    let mut series = BarSeries::new(period, "end", policy)?;
    let history_last = history.bars.last().map(|bar| bar.datetime.clone());
    let mut recorded = incoming.into_values().peekable();
    for mut bar in history.bars {
        bar.datetime = live_market::stamp(live_market::time(&bar.datetime)?);
        while recorded.peek().is_some_and(|live| live.datetime <= bar.datetime) {
            let live = recorded.next().expect("next live bar");
            if live.datetime == bar.datetime {
                bar = live;
                break;
            }
            series.push(live)?;
        }
        series.push(bar)?;
    }
    for bar in recorded { series.push(bar)?; }
    if history_last.as_deref().is_some_and(|last| last > last_label.as_str()) {
        closed_through = history_last.as_deref().map(live_market::time).transpose()?;
    }
    let (bars, group) = series.finish(range, closed_through)?;
    let broker = if jpx {
        broker_symbol(&tx, &last_contract).map_err(|e| e.to_string())?
    } else {
        last_contract.clone()
    };
    Ok(json!({"bars":bars,"kind":"minute","amount":period.amount(),"source":"live","policy":policy,
        "zone":zone,"broker_symbol":broker,"live_symbol":last_contract,"overview_group":group,
        "continuous_product":product,"continuous_mxf":product=="MXF","last_tick":last_seen.map(live_market::stamp),
        "history_issue":history_result.err(),"history_compatible":history_compatible}))
}

fn request(
    root: &Path,
    path: &Path,
    history_path: &Path,
    symbol: &str,
    profile: &str,
    contract: &str,
    history_only: bool,
    period: Period,
    range: DateRange,
) -> Result<Value, String> {
    let mut products = BTreeMap::new();
    let mut live = Value::Null;
    let mut history = Value::Null;
    let product = symbol.to_ascii_uppercase();
    let continuous = matches!(product.as_str(), "TXF1" | "TXF" | "MXF" | "TMF" | "CDF" | "225M") && !history_only && matches!(period, Period::Auto | Period::Minute(_));
    // Read the two stores independently: loss of one source must not blank the other.
    let mut issue = Vec::new();
    for (file, is_history) in [(path, false), (history_path, true)] {
        if !file.is_file() {
            continue;
        }
        let result = (|| -> Result<(), String> {
            let mut db = read_db(file)?;
            let tx = db.transaction().map_err(|e| e.to_string())?;
            catalog(&tx, &mut products).map_err(|e| e.to_string())?;
            if !symbol.is_empty() && !continuous {
                if is_history {
                    if range.active() || live["bars"].as_array().is_none_or(|bars| bars.is_empty())
                    {
                        history = historical(&tx, symbol, period, range)?;
                    }
                } else {
                    if !history_only {
                        let product = match symbol.to_ascii_uppercase().as_str() {
                            "TXF" | "TXF1" => Some("TXF"),
                            "MXF" | "MXF1" => Some("MXF"),
                            "TMF" | "TMF1" => Some("TMF"),
                            "CDF" => Some("CDF"),
                            _ => None,
                        };
                        let actual = if let Some(product) = product {
                            live_strategy::domestic_near_quote(root, product)?
                        } else if symbol.eq_ignore_ascii_case("225M") && table(&tx, "live_sources").map_err(|e| e.to_string())? {
                            tx.query_row(
                                "SELECT s.symbol FROM live_sources s JOIN datasets d ON d.id=s.tick_id
                                 WHERE s.provider='kgi_quotecom' AND s.timezone='Asia/Tokyo'
                                 AND s.symbol GLOB '225M-[0-9][0-9][0-9][0-9][0-9][0-9]' AND d.row_count>0
                                 ORDER BY d.last_datetime DESC, s.symbol DESC LIMIT 1",
                                [], |r| r.get::<_, String>(0),
                            ).optional().map_err(|e| e.to_string())?
                        } else { None };
                        let mut shown_symbol = actual.as_deref().unwrap_or(symbol);
                        live = snapshot(&tx, shown_symbol, period, range)?;
                        if shown_symbol != symbol && live["bars"].as_array().is_none_or(|bars| bars.is_empty()) {
                            shown_symbol = symbol;
                            live = snapshot(&tx, shown_symbol, period, range)?;
                        }
                        if live.is_object() { live["live_symbol"] = json!(shown_symbol); }
                    }
                }
            }
            tx.commit().map_err(|e| e.to_string())
        })();
        if let Err(error) = result {
            issue.push(error);
        }
    }
    if continuous {
        match continuous_near(root, path, history_path, &product, period, range) {
            Ok(result) => {
                if let Some(error) = result["history_issue"].as_str() { issue.push(error.into()); }
                live = result;
            }
            Err(error) => issue.push(error),
        }
        let separate_history = product == "TXF1" && range.active() && live["history_compatible"] != true;
        if (live["bars"].as_array().is_none_or(|bars| bars.is_empty()) || separate_history) && history_path.is_file() {
            match read_db(history_path).and_then(|db| historical(&db, symbol, period, range)) {
                Ok(data) => history = data,
                Err(error) => issue.push(error),
            }
        }
    }
    let live_has_data = live["bars"].as_array().is_some_and(|rows| !rows.is_empty());
    // When the requested dates reach further back than the live store, let
    // the historical series cover the start rather than hiding it.
    let history_first = text(&history["bars"][0][0]);
    let prefer_history =
        range.active() && !history_first.is_empty() && history_first < text(&live["bars"][0][0]);
    let show_live = live_has_data && !prefer_history;
    let mut data = if show_live { live } else { history };
    if prefer_history && live_has_data { data["range_prefers_history"] = json!(true); }
    if show_live {
        let selected = if !contract.is_empty() {
            contract
        } else if !text(&data["broker_symbol"]).is_empty() {
            text(&data["broker_symbol"])
        } else {
            symbol
        };
        let fills = annotations(root, &data, profile, selected);
        data["trade_symbol"] = json!(selected);
        match fills {
            Ok(fills) => {
                data["fills"] = fills["fills"].clone();
                data["unmapped"] = fills["unmapped"].clone();
            }
            Err(error) => {
                data["fills"] = json!({});
                data["fill_error"] = json!(error);
            }
        }
    }
    Ok(
        json!({"symbols":products.keys().collect::<Vec<_>>(),"products":products,"data":data,"issue":issue.join("；")}),
    )
}

#[derive(GodotClass)]
#[class(base=RefCounted,init)]
struct LiveChartBridge {
    base: Base<RefCounted>,
    pending: Option<mpsc::Receiver<String>>,
}

#[godot_api]
impl LiveChartBridge {
    #[func]
    fn start(
        &mut self,
        path: GString,
        history: GString,
        symbol: GString,
        profile: GString,
        contract: GString,
        history_only: bool,
        unit: GString,
        amount: i64,
        from: GString,
        to: GString,
    ) -> bool {
        if self.pending.is_some() {
            return false;
        }
        let (tx, rx) = mpsc::channel();
        self.pending = Some(rx);
        let [path, history, symbol, profile, contract] =
            [path, history, symbol, profile, contract].map(|v| v.to_string());
        std::thread::spawn(move || {
            let root = workbench::root();
            let result = Period::parse(&unit.to_string(), amount).and_then(|period| {
                let range = DateRange::parse(&from.to_string(), &to.to_string())?;
                request(
                    &root,
                    &root.join(path),
                    &root.join(history),
                    &symbol,
                    &profile,
                    &contract,
                    history_only,
                    period,
                    range,
                )
            });
            let body = match result {
                Ok(data) => json!({"ok":true,"data":data}),
                Err(error) => json!({"ok":false,"error":error}),
            };
            let _ = tx.send(body.to_string());
        });
        true
    }

    #[func]
    fn poll(&mut self) -> GString {
        let Some(rx) = &self.pending else {
            return GString::new();
        };
        match rx.try_recv() {
            Ok(data) => {
                self.pending = None;
                GString::from(&data)
            }
            Err(mpsc::TryRecvError::Empty) => GString::new(),
            Err(_) => {
                self.pending = None;
                GString::from("{\"ok\":false,\"error\":\"即時圖表讀取已中斷\"}")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_lists_monthly_contracts_under_one_continuous_product() {
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch("
            CREATE TABLE live_sources(symbol TEXT);
            INSERT INTO live_sources VALUES
                ('TXFL6'), ('TXFA7'), ('mxfl6'), ('MXFA7'), ('CDFL6'), ('TMFL6'),
                ('225M-202612'), ('225M-202703'), ('OTHER42');
            CREATE TABLE qm_instruments(symbol TEXT, config TEXT);
        ").unwrap();
        for (symbol, name) in [("MXF", "小臺近月"), ("MXFL6", "小臺十二月")] {
            db.execute("INSERT INTO qm_instruments VALUES (?1,?2)",
                params![symbol, json!({"name": name}).to_string()]).unwrap();
        }
        let mut products = BTreeMap::new();
        catalog(&db, &mut products).unwrap();
        assert_eq!(products.keys().map(String::as_str).collect::<Vec<_>>(),
            ["225M", "CDF", "MXF", "OTHER42", "TMF", "TXF1"]);
        assert_eq!(products["MXF"], "小臺近月");
        assert_eq!(products["TMF"], "微型臺指期貨");
        assert_eq!(products["225M"], "小型日經225期貨");
    }

    #[test]
    fn explicit_minutes_preserve_more_than_five_thousand_candles_and_the_start_date() {
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch("
            CREATE TABLE records(dataset_id INTEGER,ordinal INTEGER,datetime TEXT,payload TEXT);
            CREATE INDEX records_time ON records(dataset_id,datetime,ordinal);
            WITH RECURSIVE minutes(i) AS (VALUES(1) UNION ALL SELECT i+1 FROM minutes WHERE i<30010)
            INSERT INTO records SELECT 1,i,strftime('%Y-%m-%dT%H:%M:%S','2024-01-01',printf('+%d minutes',i)),
            '{\"open\":100,\"high\":102,\"low\":99,\"close\":101,\"volume\":1}' FROM minutes;
        ").unwrap();
        let period = Period::Minute(5);
        let range = DateRange::parse("2024-01-01", "2024-01-22").unwrap();
        let mut series = BarSeries::new(period, "end", "").unwrap();
        read_closed(&db, 1, period, range, &mut series).unwrap();
        let (bars, group) = series.finish(range, None).unwrap();
        assert_eq!(group, 1);
        assert_eq!(bars.len(), 6002);
        assert_eq!(bars[0][0], "2024-01-01T00:05:00.000000000");
        assert_eq!(bars[0][5], 5.0);
        assert_eq!(bars.last().unwrap()[0], "2024-01-21T20:10:00.000000000");
    }

    #[test]
    fn a_tick_only_product_displays_one_second_candles() {
        let root = workbench::root();
        let path = root
            .join("outputs")
            .join(crate::strategy_library::fresh_id())
            .with_extension("sqlite");
        let mut store = live_market::Store::open(&root, &path, "Asia/Taipei", true).unwrap();
        for (n, time, price, qty) in [
            (1, "2026-11-19T23:59:58.900", "19998", "1"),
            (2, "2026-11-19T23:59:59.900", "19999", "2"),
            (3, "2026-11-20T08:45:01.010", "20000", "1"),
            (4, "2026-11-20T08:45:01.110", "20005", "2"),
        ] {
            let tick = json!({"protocol_version":1,"provider_id":"fixture","connection_id":"read","connection_epoch":"one",
                "event_id":n,"type":"trade","instrument":"TICK_ONLY","event_time":time,"price":price,"quantity":qty});
            store.ingest(&tick.to_string()).unwrap();
        }
        drop(store);
        let fixture = Connection::open(&path).unwrap();
        fixture
            .execute(
                "DELETE FROM market_sources WHERE symbol='TICK_ONLY' AND kind='minute'",
                [],
            )
            .unwrap();
        drop(fixture);
        let c = read_db(&path).unwrap();
        let view = historical(&c, "TICK_ONLY", Period::Auto, DateRange::default()).unwrap();
        assert_eq!(view["kind"], "second");
        assert_eq!(
            view["bars"],
            json!([[
                "2026-11-20T08:45:02",
                "20000",
                "20005",
                "20000",
                "20005",
                "3",
                false
            ]])
        );
        let range = DateRange::parse("2026-11-19", "2026-11-19").unwrap();
        let ticks = historical(&c, "TICK_ONLY", Period::Tick, range).unwrap();
        assert_eq!(ticks["bars"].as_array().unwrap().len(), 2);
        assert_eq!(ticks["bars"][1][0], "2026-11-19T23:59:59.900000000");
        let seconds = historical(&c, "TICK_ONLY", Period::Second(1), range).unwrap();
        assert_eq!(seconds["bars"].as_array().unwrap().len(), 1);
        assert_eq!(seconds["bars"][0][0], "2026-11-19T23:59:59");
        drop(c);
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn cdf_continuous_chart_rolls_at_stock_night_open() {
        let root = workbench::root();
        let path = root.join("outputs").join(format!("cdf-roll-{}", crate::strategy_library::fresh_id())).with_extension("sqlite");
        let range = DateRange::parse("2026-12-16", "2026-12-17").unwrap();
        let mut store = live_market::Store::open(&root, &path, "Asia/Taipei", true).unwrap();
        for (n, symbol, at, price, accepted) in [
            (1, "CDFL6", "2026-12-16T13:29:30", "1000", true),
            (2, "CDFL6", "2026-12-16T13:30:00", "1001", true),
            (3, "CDFA7", "2026-12-16T13:32:00", "1020", true),
            (4, "CDFA7", "2026-12-16T17:24:59", "1030", false),
            (5, "CDFL6", "2026-12-16T17:25:01", "1010", true),
            (6, "CDFA7", "2026-12-16T17:25:01", "1040", true),
            (7, "CDFA7", "2026-12-17T04:59:01", "1050", true),
        ] {
            let tick = json!({"protocol_version":1,"provider_id":"kgi_quotecom","connection_id":"fixture",
                "connection_epoch":"one","event_id":n,"type":"trade","instrument":symbol,
                "event_time":at,"price":price,"quantity":"1"});
            assert_eq!(store.ingest(&tick.to_string()).is_ok(), accepted, "{symbol} {at}");
        }
        let watermark = json!({"protocol_version":1,"provider_id":"kgi_quotecom","connection_id":"fixture",
            "connection_epoch":"one","event_id":8,"type":"watermark","event_time":"2026-12-17T05:00:00"});
        store.ingest(&watermark.to_string()).unwrap();
        drop(store);
        let unused_history = path.with_extension("no-history");
        let chart = continuous_near(&root, &path, &unused_history, "CDF", Period::Minute(1), range).unwrap();
        let bars = chart["bars"].as_array().unwrap();
        assert_eq!(chart["live_symbol"], "CDFA7");
        assert_eq!(bars.len(), 3);
        for (bar, (at, close)) in bars.iter().zip([
            ("2026-12-16T13:30:00.000000000", 1001.0),
            ("2026-12-16T17:26:00.000000000", 1040.0),
            ("2026-12-17T05:00:00.000000000", 1050.0),
        ]) {
            assert_eq!(bar[0], at);
            assert_eq!(bar[4].as_f64(), Some(close));
        }
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn jpx_minutes_keep_history_and_replace_overlaps_with_live_data() {
        let root = workbench::root();
        let path = root.join("outputs").join(format!("jpx-history-{}", crate::strategy_library::fresh_id())).with_extension("sqlite");
        let history_path = path.with_extension("history.sqlite");
        let history = Connection::open(&history_path).unwrap();
        history.execute_batch("
            CREATE TABLE market_sources(symbol TEXT, kind TEXT, dataset_id INTEGER);
            CREATE TABLE datasets(id INTEGER, timezone TEXT, timestamp_role TEXT, row_count INTEGER);
            CREATE TABLE records(dataset_id INTEGER, ordinal INTEGER, datetime TEXT, payload TEXT);
            INSERT INTO market_sources VALUES('225M','minute',1);
            INSERT INTO datasets VALUES(1,'Asia/Tokyo','end',2);
        ").unwrap();
        for (ordinal, at) in [(1, "2026-11-24T08:45:00"), (2, "2026-11-24T08:46:00")] {
            let bar = json!({"open":50000,"high":50000,"low":50000,"close":50000,"volume":1});
            history.execute("INSERT INTO records VALUES(1,?1,?2,?3)",
                params![ordinal, at, bar.to_string()]).unwrap();
        }
        drop(history);
        let mut store = live_market::Store::open(&root, &path, "Asia/Tokyo", false).unwrap();
        for (n, source, at, price) in [
            (1, "202612", "2026-11-24T08:45:30", "50100"),
            (2, "202612", "2026-11-24T08:47:00", "50105"),
            (3, "202703", "2026-11-24T08:46:30", "50200"),
        ] {
            let tick = json!({"protocol_version":1,"provider_id":"kgi_quotecom",
                "connection_id":format!("fixture:{source}"),"connection_epoch":"one",
                "event_id":n,"type":"trade","instrument":format!("225M-{source}"),
                "provider_symbol":if source=="202612" { "225M-Z26" } else { "225M-H27" },
                "event_time":at,"price":price,"quantity":"1"});
            store.ingest(&tick.to_string()).unwrap();
        }
        drop(store);
        let range = DateRange::parse("2026-11-24", "2026-11-24").unwrap();
        let view = request(&root, &path, &history_path, "225M", "", "", false, Period::Minute(1), range).unwrap();
        let data = &view["data"];
        assert_eq!(data["zone"], "Asia/Tokyo");
        assert_eq!(data["broker_symbol"], "225M-H27");
        assert_eq!(data["bars"].as_array().unwrap().len(), 3);
        assert_eq!(data["bars"][0][4], 50000.0);
        assert_eq!(data["bars"][1][4], 50100.0);
        assert_eq!(data["bars"][2][4], 50200.0);
        let overview = request(&root, &path, &history_path, "225M", "", "", false, Period::Minute(5), DateRange::default()).unwrap();
        assert_eq!(overview["data"]["bars"][0][4], 50000.0);
        assert_eq!(overview["data"]["bars"][1][4], 50200.0);
        let historical_only = request(&root, &path, &history_path, "225M", "", "", true, Period::Minute(1), range).unwrap();
        assert_eq!(historical_only["data"]["source"], "history");
        assert_eq!(historical_only["data"]["bars"][1][4], 50000.0);
        std::fs::remove_file(path).unwrap();
        std::fs::remove_file(history_path).unwrap();
    }

    #[test]
    fn night_fill_uses_calendar_time_even_when_trading_date_is_later() {
        let mut fill = json!({"session":"P","trade_date":"20261123","time":"235959500",
            "received_at":"2026-11-21T00:00:01","recovered":false});
        let at = broker_time(&fill).unwrap();
        assert_eq!(live_market::stamp(at), "2026-11-20T23:59:59.500000000");
        fill["recovered"] = json!(true);
        assert!(broker_time(&fill).is_none());
        fill["report_time2"] = json!("20261120235959500");
        assert_eq!(broker_time(&fill), Some(at));
    }
}
