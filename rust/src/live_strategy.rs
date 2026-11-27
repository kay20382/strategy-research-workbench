//! Bar-close execution against the live domestic futures gateway.
use crate::{
    backtest::{self, Candle},
    database::Payload,
    live_market,
    powerlanguage::{Action, Kind, Order, Program, Runtime, Stmt},
    strategy_library as library, strategy_research,
    trading::Session,
    workbench,
};
use chrono::{Datelike, NaiveDateTime, Utc, Weekday};
use godot::prelude::*;
use rusqlite::{params, Connection, OpenFlags, OptionalExtension};
use serde_json::{json, Value};
use std::{
    fs::{self, OpenOptions},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc,
    },
    time::{Duration, Instant, SystemTime},
};

const MAX_BARS: usize = 1800;
const STOP: &str = "已停止策略。請在交易回報核對已送出的委託及部位。";

struct Lease(PathBuf);
impl Drop for Lease {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

fn allocate(root: &Path) -> Result<Lease, String> {
    let path = root.join("data/live-strategy.lock");
    OpenOptions::new().write(true).create_new(true).open(&path)
        .map_err(|e|format!("無法取得策略執行鎖。請確認其他工作台是否仍在執行，並在異常結束後先核對券商回報。{e}"))?;
    Ok(Lease(path))
}

fn validate(body: &[Stmt]) -> Result<(), String> {
    for node in body {
        match node {
            Stmt::Order(order) if order.kind != Kind::Market || order.total => {
                return Err(
                    "即時執行目前支援 next bar at market 單口委託。此策略有其他委託型態。".into(),
                )
            }
            Stmt::If(_, yes, no) => {
                validate(yes)?;
                validate(no)?;
            }
            Stmt::For(_, _, _, _, body) => validate(body)?,
            _ => {}
        }
    }
    Ok(())
}

fn plan(orders: &[Order], position: i64) -> Result<Option<(Action, bool)>, String> {
    let mut planned = None;
    for order in orders {
        let active = match order.action {
            Action::Buy => position <= 0,
            Action::SellShort => position >= 0,
            Action::Sell => position > 0,
            Action::BuyToCover => position < 0,
        };
        if !active {
            continue;
        }
        let entry = matches!(order.action, Action::Buy | Action::SellShort);
        if (entry && position != 0) || order.quantity.unwrap_or(1) != 1 || planned.is_some() {
            return Err("訊號包含反手、多口或同棒複數委託，策略已停止。".into());
        }
        planned = Some((order.action, entry));
    }
    Ok(planned)
}

fn text(value: &Value) -> &str {
    value.as_str().unwrap_or_default().trim()
}
fn numeric(value: &Value) -> Option<f64> {
    value
        .as_f64()
        .or_else(|| text(value).parse().ok())
        .filter(|v| v.is_finite())
}
fn local_now() -> NaiveDateTime {
    chrono::DateTime::<Utc>::from(SystemTime::now()).naive_utc() + chrono::TimeDelta::hours(8)
}
fn sqlite<T>(result: rusqlite::Result<T>) -> Result<T, String> {
    result.map_err(|e| e.to_string())
}

fn market_path(root: &Path) -> PathBuf {
    std::env::var_os("STRATEGY_LIVE_MARKET_DB")
        .filter(|v| !v.is_empty())
        .map(|v| root.join(v))
        .unwrap_or_else(|| root.join("data/market-live.sqlite"))
}

fn monthly_expiry(first: chrono::NaiveDate, overrides: &Value) -> Option<chrono::NaiveDate> {
    let key = first.format("%Y%m").to_string();
    if let Some(value) = overrides.get(&key) {
        let day = chrono::NaiveDate::parse_from_str(value.as_str()?, "%Y-%m-%d").ok()?;
        return (day.year() == first.year() && day.month() == first.month()).then_some(day);
    }
    (15..=21).filter_map(|day| first.with_day(day))
        .find(|day| day.weekday() == Weekday::Wed)
}

pub(crate) fn near_month(product: &str, now: NaiveDateTime, overrides: &Value) -> Option<(String, String, bool)> {
    // These products share the ordinary TAIFEX monthly settlement schedule.
    if !matches!(product, "TXF" | "MXF" | "TMF") {
        return None;
    }
    monthly_quote_contract(product, now, overrides)
}

pub(crate) fn monthly_quote_contract(product: &str, now: NaiveDateTime, overrides: &Value) -> Option<(String, String, bool)> {
    let (night_hour, night_minute) = match product {
        "TXF" | "MXF" | "TMF" => (15, 0),
        "CDF" => (17, 25),
        _ => return None,
    };
    let first = now.date().with_day(1)?;
    let expiry = monthly_expiry(first, overrides)?;
    let close = expiry.and_hms_opt(13, 30, 0)?;
    let night = expiry.and_hms_opt(night_hour, night_minute, 0)?;
    let month = if now >= night {
        first.checked_add_months(chrono::Months::new(1))?
    } else {
        first
    };
    let code = char::from(b'A' + month.month0() as u8);
    Some((
        month.format("%Y%m").to_string(),
        format!("{product}{code}{}", month.year().rem_euclid(10)),
        now >= close && now < night,
    ))
}

pub(crate) fn expiry_overrides(root: &Path) -> Result<Value, String> {
    let path = root.join("data/taifex-monthly-expiries.json");
    if !path.exists() { return Ok(json!({})); }
    let dates = workbench::read_json(&path)?;
    if !dates.is_object() { return Err("期交所最後交易日設定須為 JSON 物件".into()); }
    Ok(dates)
}

pub(crate) fn domestic_near_quote(root: &Path, product: &str) -> Result<Option<String>, String> {
    let dates = expiry_overrides(root)?;
    Ok(monthly_quote_contract(product, local_now(), &dates).map(|(_, contract, _)| contract))
}

fn front_contract(path: &Path, product: &str, now: NaiveDateTime, overrides: &Value) -> Result<Value, String> {
    let Some((month, contract, paused)) = near_month(product, now, overrides) else {
        return Ok(json!({"supported":false}));
    };
    let candidates = contract_sources(path, &contract)?;
    let instrument = if candidates.len() == 1 { candidates[0].as_str() } else { "" };
    Ok(json!({"supported":true,"maturity":month,"contract":contract,"instrument":instrument,"ambiguous":candidates.len()>1,"paused":paused}))
}

fn contract_sources(path: &Path, contract: &str) -> Result<Vec<String>, String> {
    let mut candidates = Vec::new();
    if path.is_file() {
        let db = sqlite(Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_ONLY,
        ))?;
        sqlite(db.busy_timeout(Duration::from_millis(150)))?;
        let has_live:bool=sqlite(db.query_row("SELECT count(*)=2 FROM sqlite_master WHERE type='table' AND name IN ('live_sources','live_events')",[],|r|r.get(0)))?;
        if has_live {
            let mut query=sqlite(db.prepare("SELECT DISTINCT s.symbol FROM (SELECT provider,connection,body FROM live_events ORDER BY id DESC LIMIT 20000) e
                JOIN live_sources s ON s.symbol=json_extract(e.body,'$.instrument') AND s.provider=e.provider AND s.connection=e.connection
                WHERE e.provider='kgi_quotecom' AND s.timezone='Asia/Taipei' AND s.session_policy='taifex_regular'
                  AND json_extract(e.body,'$.provider_symbol')=?1 COLLATE NOCASE ORDER BY s.symbol"))?;
            let rows = sqlite(query.query_map([contract], |r| r.get::<_, String>(0)))?;
            candidates = sqlite(rows.collect())?;
        }
    }
    Ok(candidates)
}

struct Quotes {
    db: Connection,
    dataset: i64,
    symbol: String,
    feed: String,
    windows: crate::tick_bars::Aggregator,
}
impl Quotes {
    fn open(path: &Path, symbol: &str) -> Result<Option<Self>, String> {
        if !path.is_file() { return Ok(None); }
        let db = sqlite(Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_ONLY,
        ))?;
        sqlite(db.busy_timeout(Duration::from_millis(300)))?;
        let has_live:bool=sqlite(db.query_row("SELECT count(*)=2 FROM sqlite_master WHERE type='table' AND name IN ('live_sources','live_events')",[],|r|r.get(0)))?;
        if !has_live { return Ok(None); }
        let Some(dataset)=sqlite(db.query_row("SELECT minute_id FROM live_sources WHERE symbol=?1 AND provider='kgi_quotecom' AND timezone='Asia/Taipei' AND session_policy='taifex_regular'",
            [symbol],|r|r.get(0)).optional())? else { return Ok(None); };
        let Some(feed)=sqlite(db.query_row("SELECT json_extract(body,'$.provider_symbol') FROM (SELECT provider,connection,body FROM live_events ORDER BY id DESC LIMIT 20000) e JOIN live_sources s ON s.symbol=?1 AND e.provider=s.provider AND e.connection=s.connection WHERE json_extract(e.body,'$.instrument')=?1 AND json_type(e.body,'$.provider_symbol')='text' LIMIT 1",[symbol],|r|r.get::<_,String>(0)).optional())?
            .filter(|s|!s.is_empty()) else { return Ok(None); };
        Ok(Some(Self {
            db,
            dataset,
            symbol: symbol.into(),
            feed,
            windows: crate::tick_bars::Aggregator::with_sessions(60)?,
        }))
    }

    fn read(&self, after: Option<i64>, limit: usize) -> Result<Vec<(i64, Candle)>, String> {
        let sql = if after.is_some() {
            "SELECT ordinal,datetime,payload FROM records WHERE dataset_id=?1 AND ordinal>?2 ORDER BY ordinal LIMIT ?3"
        } else {
            "SELECT ordinal,datetime,payload FROM records WHERE dataset_id=?1 AND ordinal>?2 ORDER BY ordinal DESC LIMIT ?3"
        };
        let mut query = sqlite(self.db.prepare(sql))?;
        let rows = sqlite(query.query_map(
            params![self.dataset, after.unwrap_or(0), limit as i64],
            |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                ))
            },
        ))?;
        let mut bars = Vec::new();
        for row in rows {
            let (id, time, body) = sqlite(row)?;
            let payload: Payload = serde_json::from_str(&body).map_err(|e| e.to_string())?;
            bars.push((id, backtest::candle(time, payload)?));
        }
        if after.is_none() {
            bars.reverse();
        }
        Ok(bars)
    }

    fn next_minute(&self, previous: &str) -> Result<bool, String> {
        let previous = live_market::time(previous)?;
        let now = local_now();
        if now < previous || (now - previous).num_milliseconds() > 18_000 {
            return Ok(false);
        }
        let Some((start, end)) = self.windows.window(now) else {
            return Ok(false);
        };
        if start != previous.and_utc().timestamp() {
            return Ok(false);
        }
        let end = live_market::stamp(
            chrono::DateTime::from_timestamp(end, 0)
                .ok_or("行情時間超限")?
                .naive_utc(),
        );
        let tag:Option<String>=sqlite(self.db.query_row("SELECT json_extract(e.body,'$.provider_symbol') FROM live_pending p JOIN live_events e ON e.id=p.event_id WHERE p.symbol=?1 AND p.bar_end=?2 ORDER BY p.event_id DESC LIMIT 1",params![self.symbol,end],|r|r.get(0)).optional())?.flatten();
        Ok(tag.is_some_and(|tag| tag.eq_ignore_ascii_case(&self.feed)))
    }
}

struct ExecutionBars {
    windows: crate::tick_bars::Aggregator,
    partial: Option<(i64, Candle)>,
}

impl ExecutionBars {
    fn new(period: usize) -> Result<Self, String> {
        Ok(Self { windows: crate::tick_bars::Aggregator::with_sessions(period as i64 * 60)?, partial: None })
    }

    fn push(&mut self, mut minute: Candle, strict: bool) -> Result<Option<Candle>, String> {
        let close = live_market::time(&minute.datetime)?;
        let Some((start, end)) = self.windows.window(close - chrono::TimeDelta::nanoseconds(1)) else {
            return Err("即時 1 分 K 超出策略商品時段。".into());
        };
        if let Some((previous_end, previous)) = &mut self.partial {
            if *previous_end == end {
                if strict && (close - live_market::time(&previous.datetime)?).num_seconds() != 60 {
                    return Err("週期 K 棒缺少來源分 K，策略已停止。".into());
                }
                minute.open = previous.open;
                minute.high = minute.high.max(previous.high);
                minute.low = minute.low.min(previous.low);
                minute.volume += previous.volume;
                minute.source_rows += previous.source_rows;
            } else if strict {
                return Err("上一根週期 K 棒未完成，策略已停止。".into());
            }
        }
        if close.and_utc().timestamp() == end {
            self.partial = None;
            if minute.source_rows == ((end - start) / 60) as usize {
                return Ok(Some(minute));
            }
            if strict { return Err("週期 K 棒缺少來源分 K，策略已停止。".into()); }
        } else {
            self.partial = Some((end, minute));
        }
        Ok(None)
    }

    fn warmup(quotes: &Quotes, count: usize, period: usize) -> Result<(Self, i64, Vec<Candle>), String> {
        let mut series = Self::new(period)?;
        let mut cursor = 0;
        let mut bars = Vec::new();
        for (id, minute) in quotes.read(None, (count + 2) * period)? {
            cursor = id;
            if let Some(bar) = series.push(minute, false)? { bars.push(bar); }
        }
        bars.drain(..bars.len().saturating_sub(count));
        Ok((series, cursor, bars))
    }
}

trait Broker {
    fn submit(&mut self, value: Value) -> Result<(), String>;
    fn poll(&mut self) -> Result<(Vec<Value>, bool), String>;
}
impl Broker for Session {
    fn submit(&mut self, value: Value) -> Result<(), String> {
        Session::submit(self, value)
    }
    fn poll(&mut self) -> Result<(Vec<Value>, bool), String> {
        Session::poll(self)
    }
}

fn wait_for(
    broker: &mut impl Broker,
    stop: &AtomicBool,
    timeout: Duration,
    mut handle: impl FnMut(&Value) -> Result<Option<Value>, String>,
) -> Result<Value, String> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if stop.load(Ordering::Acquire) {
            return Err(STOP.into());
        }
        let (events, active) = broker.poll()?;
        for event in events {
            if event["type"] == "status" && matches!(text(&event["data"]["state"]), "error" | "disconnected" | "reconnecting") {
                return Err(text(&event["data"]["message"]).into());
            }
            if let Some(result) = handle(&event)? {
                return Ok(result);
            }
        }
        if !active {
            return Err("券商交易連線已中斷，請核對回報與部位。".into());
        }
        std::thread::sleep(Duration::from_millis(40));
    }
    Err("券商回報逾時。策略已停止，請先核對委託與部位。".into())
}

fn query(broker: &mut impl Broker, stop: &AtomicBool, name: &str) -> Result<Value, String> {
    let id = format!("pl-{name}-{}", library::fresh_id());
    broker.submit(json!({"id":id,"action":name}))?;
    wait_for(broker, stop, Duration::from_secs(6), |event| {
        let d = &event["data"];
        if d["id"] != id {
            return Ok(None);
        }
        if event["type"] == "reply" || (event["type"] == "query" && d["code"] != 0) {
            return Err(format!("券商{name}查詢失敗，請核對交易回報。"));
        }
        Ok((event["type"] == "query").then(|| d.clone()))
    })
}

fn verify_position(
    broker: &mut impl Broker,
    stop: &AtomicBool,
    position: i64,
    product: &str,
    maturity: &str,
) -> Result<(), String> {
    let snapshot = query(broker, stop, "positions")?;
    let rows = snapshot["rows"].as_array().ok_or("券商部位格式無法核對")?;
    if snapshot["received_rows"].as_u64() != Some(rows.len() as u64) {
        return Err("部位回報有未核對的帳戶資料。".into());
    }
    let mut active = vec![];
    for row in rows {
        let qty = numeric(&row["quantity"]).ok_or("券商部位口數無法核對")?;
        if qty != 0.0 {
            active.push((row, qty));
        }
    }
    let matches = if position == 0 {
        active.is_empty()
    } else if let [(row, qty)] = active.as_slice() {
        *qty == 1.0
            && text(&row["symbol"]).eq_ignore_ascii_case(product)
            && text(&row["maturity"]) == maturity
            && text(&row["side"]) == if position == 1 { "B" } else { "S" }
    } else {
        false
    };
    if !matches {
        return Err("券商部位與本策略狀態不一致，已停止執行。請核對交易帳戶。".into());
    }
    Ok(())
}

fn send_order(
    broker: &mut impl Broker,
    stop: &AtomicBool,
    action: Action,
    entry: bool,
    product: &str,
    maturity: &str,
    feed: &str,
) -> Result<f64, String> {
    if stop.load(Ordering::Acquire) {
        return Err(STOP.into());
    }
    let id = format!("live-pl-{}", library::fresh_id());
    broker.submit(json!({"id":id,"action":"place","confirmed":true,"product":product,"maturity":maturity,
        "expected_symbol":feed,"side":if matches!(action,Action::Buy|Action::BuyToCover){"buy"}else{"sell"},
        "position_effect":if entry {"open"}else{"close"},"price_type":"market","time_in_force":"ioc","quantity":"1"}))?;
    let fill = wait_for(broker, stop, Duration::from_secs(15), |event| {
        let d = &event["data"];
        if event["type"] == "fill" && text(&d["symbol"]).eq_ignore_ascii_case(feed) && d["id"] != id
        {
            return Err("收到同契約未能對應的成交，請核對券商回報。".into());
        }
        if d["id"] != id {
            return Ok(None);
        }
        if (event["type"] == "reply" && d["state"] != "submitted")
            || (event["type"] == "order_ack" && d["code"] != 0)
            || (event["type"] == "order_report" && numeric(&d["code"]) != Some(0.0))
            || (event["type"] == "transport_ack" && d["received"] == false)
        {
            return Err(format!("委託未確認（{id}），請核對交易回報。"));
        }
        if event["type"] == "fill" {
            return Ok(Some(d.clone()));
        }
        Ok(None)
    })?;
    if numeric(&fill["quantity"]) != Some(1.0)
        || numeric(&fill["remaining"]) != Some(0.0)
        || !text(&fill["symbol"]).eq_ignore_ascii_case(feed)
    {
        return Err("成交口數或契約無法核對，策略已停止。".into());
    }
    numeric(&fill["price"]).ok_or("成交價無法核對".into())
}

fn evaluate(
    program: &Program,
    bars: &[Candle],
    state: &[(i64, f64)],
    warmup: usize,
) -> Result<Vec<Order>, String> {
    let mut runtime = Runtime::new(program, bars);
    runtime.bar_offset = warmup;
    let mut orders = vec![];
    for (i, &(position, price)) in state.iter().enumerate().skip(warmup) {
        runtime.execution.ticks = Some(bars[i].volume);
        orders = runtime.step(i, position, price, position.abs())?;
    }
    Ok(orders)
}

fn wait_for_quotes(path: &Path, instrument: &str, contract: &str, warmup: usize, period: usize,
    stop: &AtomicBool, log: &dyn Fn(&str), expired: &dyn Fn() -> bool) -> Result<Option<Quotes>, String> {
    let mut last_notice = String::new();
    while !stop.load(Ordering::Acquire) {
        if expired() { return Ok(None); }
        let candidates = if instrument.is_empty() { contract_sources(path, contract)? } else { vec![instrument.to_owned()] };
        let notice = match candidates.as_slice() {
            [symbol] => match Quotes::open(path, symbol)? {
                Some(quotes) => {
                    if !quotes.feed.eq_ignore_ascii_case(contract) {
                        return Err("填寫的券商契約全碼與行情來源不符。".into());
                    }
                    let (_, _, bars) = ExecutionBars::warmup(&quotes, warmup, period)?;
                    let count = bars.len();
                    if count >= warmup { return Ok(Some(quotes)); }
                    format!("已啟用，等待商品暖機：已收 {period} 分 K {count}/{warmup} 根。")
                }
                None => format!("已啟用，等待商品的即時 1 分 K 行情。"),
            },
            [] => format!("已啟用，等待 QuoteCom 提供即時行情。"),
            _ => format!("已啟用，偵測到多個對應來源，等待唯一相符的即時行情。"),
        };
        if notice != last_notice {
            log(&notice);
            last_notice = notice;
        }
        for _ in 0..5 {
            if stop.load(Ordering::Acquire) { return Err(STOP.into()); }
            std::thread::sleep(Duration::from_millis(200));
        }
    }
    Err(STOP.into())
}

fn execution_document(root: &Path, id: &str) -> Result<library::Document, String> {
    let entry = library::catalog(root)?
        .into_iter()
        .find(|v| v.id == id && v.kind == "strategy")
        .ok_or("找不到已儲存策略")?;
    library::open(root, &entry)
}

fn execution_program(root: &Path, mut doc: library::Document, overrides: Option<&Value>) -> Result<Program, String> {
    if let Some(overrides) = overrides {
        let overrides = overrides.as_object().ok_or("策略輸入參數格式無效。")?;
        let mut inputs = serde_json::Map::new();
        if let Some(defaults) = doc.task["parameters"]["inputs"].as_object() {
            for (name, value) in defaults {
                inputs.insert(name.to_ascii_lowercase(), value.clone());
            }
        }
        for (name, value) in overrides {
            let number = value.as_f64().filter(|n| n.is_finite()).ok_or("策略輸入須為有限數字。")?;
            inputs.insert(name.to_ascii_lowercase(), json!(number));
        }
        doc.task["parameters"]["inputs"] = Value::Object(inputs);
    }
    strategy_research::compile_document(root, &doc)
}

fn run(root: &Path, args: &Value, stop: &AtomicBool, log: &dyn Fn(&str)) -> Result<(), String> {
    if args["confirmed"] != true {
        return Err("請先確認正式自動委託。".into());
    }
    let doc = execution_document(root, text(&args["strategy"]))?;
    let program = execution_program(root, doc.clone(), args.get("inputs"))?;
    validate(&program.body)?;
    let p = &doc.task["parameters"];
    if p["pyramiding"].as_str().is_some_and(|v| v != "disabled")
        || p["contracts"].as_u64().unwrap_or(1) != 1
    {
        return Err("請選擇單口、停用加碼的策略。".into());
    }
    let warmup = if let Some(value) = args.get("warmup_bars") {
        value.as_u64().ok_or("暖機棒數須為非負整數。")?
    } else {
        p["warmup_bars"].as_u64().unwrap_or(50)
    };
    let period = args.get("period_minutes").map_or(Some(1), Value::as_u64)
        .filter(|n| (1..=240).contains(n)).ok_or("商品週期須為 1～240 分 K。")? as usize;
    if warmup >= MAX_BARS as u64 {
        return Err("策略暖機超過目前即時執行上限。".into());
    }
    let warmup = warmup as usize;
    let profile = text(&args["profile"]);
    let product = text(&args["product"]);
    if profile.is_empty() || !matches!(product, "TXF" | "MXF" | "TMF") {
        return Err("請選擇交易帳戶及支援正式執行的連續商品。".into());
    }
    let _lease = allocate(root)?;
    let overrides = expiry_overrides(root)?;
    let mut waiting_for_night = false;
    while !stop.load(Ordering::Acquire) {
        let selection = || near_month(product, local_now(), &overrides);
        let (month, code, paused) = selection().ok_or("連續商品的交易日設定無效。")?;
        if paused {
            if !waiting_for_night {
                log("已啟用，結算暫停交易，等待夜盤行情接續。");
                waiting_for_night = true;
            }
            std::thread::sleep(Duration::from_millis(200));
            continue;
        }
        waiting_for_night = false;
        let expired = || selection().is_none_or(|(_, selected, paused)| paused || selected != code);
        log(&format!("已啟用，正在檢查 {product} 的即時行情。"));
        let Some(quotes) = wait_for_quotes(&market_path(root), "", &code, warmup.max(1), period, stop, log, &expired)? else {
            continue;
        };
        if expired() { continue; }
        run_contract(root, &program, quotes, profile, product, &month, warmup, period, stop, log, &expired)?;
    }
    Err(STOP.into())
}

fn verify_no_working_orders(broker: &mut Session, stop: &AtomicBool) -> Result<(), String> {
    let orders = query(broker, stop, "orders")?;
    if orders["report_ready"] != true || orders["rows"].as_array().is_none_or(|rows| !rows.is_empty()) {
        return Err("指定帳戶有工作中委託，或券商委託未完成核對。".into());
    }
    Ok(())
}

fn open_strategy_broker(
    root: &Path, profile: &str, product: &str, maturity: &str,
    position: i64, stop: &AtomicBool, log: &dyn Fn(&str),
) -> Result<Session, String> {
    log("正在登入凱基，核對交易帳戶與委託回報…");
    let mut broker = Session::start(root, profile, "live")?;
    let (mut ready, mut recovered) = (false, false);
    wait_for(&mut broker, stop, Duration::from_secs(40), |event| {
        ready |= event["type"] == "status" && event["data"]["state"] == "ready";
        recovered |= event["type"] == "recovery" && event["data"]["state"] == "complete";
        Ok((ready && recovered).then_some(Value::Null))
    })?;
    verify_position(&mut broker, stop, position, product, maturity)?;
    verify_no_working_orders(&mut broker, stop)?;
    Ok(broker)
}

fn run_contract(root: &Path, program: &Program, quotes: Quotes, profile: &str, product: &str,
    maturity: &str, warmup: usize, period: usize, stop: &AtomicBool, log: &dyn Fn(&str), expired: &dyn Fn() -> bool) -> Result<(), String> {
    if stop.load(Ordering::Acquire) {
        return Err(STOP.into());
    }
    let mut broker = open_strategy_broker(root, profile, product, maturity, 0, stop, log)?;
    let (mut series, mut cursor, mut bars) = ExecutionBars::warmup(&quotes, warmup.max(1), period)?;
    if bars.len() < warmup.max(1) {
        return Err("啟用時行情已改變，暖機資料不足。".into());
    }
    let offset = bars.len();
    let mut states = vec![(0, 0.0); offset];
    log(&format!("已啟用，等待下一根新收 {period} 分 K。"));
    let (mut position, mut price) = (0, 0.0);
    while !stop.load(Ordering::Acquire) {
        if expired() {
            if position != 0 { return Err("到期換月時策略仍有舊月部位，已停止。請核對券商部位及結算。".into()); }
            verify_position(&mut broker, stop, 0, product, maturity)?;
            verify_no_working_orders(&mut broker, stop)?;
            return Ok(());
        }
        let (events, active) = broker.poll()?;
        if events.iter().any(|e| {
            matches!(text(&e["type"]), "fill" | "order_report") && e["data"]["id"].is_null()
        }) {
            return Err("收到其他來源的委託或成交，請核對券商部位。".into());
        }
        if !active {
            if !broker.reconnectable() {
                return Err("凱基交易連線失敗，策略已停止。".into());
            }
            let delay = if broker.scheduled_reconnect() { Duration::from_secs(1) }
                else { crate::api_connection::RETRY_DELAY };
            drop(broker);
            let mut light = crate::api_connection::Indicator::new(root, "trade", profile);
            light.update("reconnecting");
            log("凱基交易連線重建中，確認部位及委託後才繼續策略。");
            let deadline = Instant::now() + delay;
            while Instant::now() < deadline {
                if stop.load(Ordering::Acquire) { return Err(STOP.into()); }
                light.heartbeat();
                std::thread::sleep(Duration::from_millis(100));
            }
            drop(light);
            if expired() {
                return Err("重連期間已到換月時間，請核對券商委託及部位後重新啟用。".into());
            }
            broker = open_strategy_broker(root, profile, product, maturity, position, stop, log)?;
            log("交易已重新連線，委託與部位核對完成。");
            continue;
        }
        if !broker.ready || events.iter().any(|event| event["type"] == "status" &&
            matches!(text(&event["data"]["state"]), "disconnected" | "reconnecting" | "error"))
        {
            // Drain disconnect-time recovery and reports before re-opening the connection.
            std::thread::sleep(Duration::from_millis(100));
            continue;
        }
        let mut new = quotes.read(Some(cursor), 2)?;
        if new.len() > 1 || bars.len() >= MAX_BARS {
            return Err("行情追不上或本次執行已達棒數上限，請核對狀態後重新啟用。".into());
        }
        if let Some((id, minute)) = new.pop() {
            if id != cursor + 1 { return Err("即時分 K 順序中斷，策略已停止。".into()); }
            let previous = series.partial.clone();
            let Some(bar) = series.push(minute, true)? else {
                cursor = id;
                continue;
            };
            let age = (local_now() - live_market::time(&bar.datetime)?).num_milliseconds();
            if (0..18_000).contains(&age) && !quotes.next_minute(&bar.datetime)? {
                series.partial = previous;
                std::thread::sleep(Duration::from_millis(200));
                continue;
            }
            if !quotes.next_minute(&bar.datetime)? {
                return Err("收棒已過有效時段，或未看到下一分鐘的即時 Tick。".into());
            }
            cursor = id;
            bars.push(bar);
            states.push((position, price));
            let orders = evaluate(program, &bars, &states, offset)?;
            if let Some((action, entry)) = plan(&orders, position)? {
                verify_position(&mut broker, stop, position, product, maturity)?;
                if expired() { continue; }
                if !quotes.next_minute(&bars.last().unwrap().datetime)? {
                    return Err("委託前的行情已過有效時段。".into());
                }
                log("策略產生委託，等待券商成交回報…");
                price = send_order(
                    &mut broker,
                    stop,
                    action,
                    entry,
                    product,
                    maturity,
                    &quotes.feed,
                )?;
                position = if !entry {
                    price = 0.0;
                    0
                } else if action == Action::Buy {
                    1
                } else {
                    -1
                };
                log("已收到本次成交回報，繼續等待下一根 K 棒。");
            } else {
                log(&format!(
                    "{} 已評估，無委託。",
                    bars.last().unwrap().datetime
                ));
            }
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    Err(STOP.into())
}

#[derive(GodotClass)]
#[class(base=RefCounted,init)]
struct LiveStrategyBridge {
    base: Base<RefCounted>,
    rx: Option<mpsc::Receiver<String>>,
    stop: Option<Arc<AtomicBool>>,
}

#[godot_api]
impl LiveStrategyBridge {
    #[func]
    fn strategy_settings(&self, id: GString) -> GString {
        let root = workbench::root();
        let data = execution_document(&root, &id.to_string()).and_then(|doc| {
            let program = strategy_research::compile_document(&root, &doc)?;
            Ok(json!({"inputs":program.inputs,
                "warmup":doc.task["parameters"]["warmup_bars"].as_u64().unwrap_or(50)}))
        });
        GString::from(&match data {
            Ok(data) => json!({"ok":true,"data":data}),
            Err(error) => json!({"ok":false,"error":error}),
        }.to_string())
    }

    #[func]
    fn nearest_contract(&self, product: GString) -> GString {
        let path = market_path(&workbench::root());
        let data = expiry_overrides(&workbench::root())
            .and_then(|dates| front_contract(&path, &product.to_string(), local_now(), &dates));
        let value = match data {
            Ok(info) => info,
            Err(error) => json!({"error":error}),
        };
        GString::from(&value.to_string())
    }

    #[func]
    fn start(&mut self, args: GString) -> bool {
        self.start_request(args, false)
    }

    #[func]
    fn start_replay(&mut self, args: GString) -> bool {
        self.start_request(args, true)
    }

    fn start_request(&mut self, args: GString, replay: bool) -> bool {
        if self.rx.is_some() {
            return false;
        }
        let args = args.to_string();
        let stop = Arc::new(AtomicBool::new(false));
        let (tx, rx) = mpsc::channel();
        self.rx = Some(rx);
        self.stop = Some(stop.clone());
        std::thread::spawn(move || {
            let log = |message: &str| {
                let _ = tx.send(json!({"type":"progress","message":message}).to_string());
            };
            if replay {
                let result = serde_json::from_str(&args).map_err(|e| e.to_string()).and_then(|args|
                    crate::foreign_strategy_replay::run(&workbench::root(), &args, &stop, |line| log(&line))
                );
                let message = match result {
                    Ok(proof) => json!({"type":"done","paper":true,"stopped":false,"proof":proof,
                        "message":"225M 模擬回放完成，已核對每筆成交與部位。"}),
                    Err(error) => json!({"type":"done","paper":true,"stopped":stop.load(Ordering::Acquire),"message":error}),
                };
                let _ = tx.send(message.to_string());
                return;
            }
            let result = serde_json::from_str(&args)
                .map_err(|e| e.to_string())
                .and_then(|args| run(&workbench::root(), &args, &stop, &log));
            let message = result.err().unwrap_or_else(|| STOP.to_owned());
            let _ = tx
                .send(json!({"type":"done","stopped":message==STOP,"message":message}).to_string());
        });
        true
    }

    #[func]
    fn stop(&self) {
        if let Some(flag) = &self.stop {
            flag.store(true, Ordering::Release);
        }
    }

    #[func]
    fn poll(&mut self) -> GString {
        let Some(rx) = &self.rx else {
            return GString::new();
        };
        let message = match rx.try_recv() {
            Ok(message) => message,
            Err(mpsc::TryRecvError::Empty) => return GString::new(),
            Err(_) => {
                json!({"type":"done","stopped":false,"message":"策略執行中斷，請核對券商回報。"})
                    .to_string()
            }
        };
        if message.contains("\"type\":\"done\"") {
            self.rx = None;
            self.stop = None;
        }
        GString::from(&message)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pair_settings_override_only_the_selected_program() {
        let doc = library::Document {
            id: "fixture".into(), name: "fixture".into(), kind: "strategy".into(),
            source: "Inputs: Length(50); if Close > Length then Buy next bar at market;".into(),
            origin: None, managed_id: None, dirty: false, result: None,
            task: json!({"parameters":{"inputs":{"Length":20}}}),
        };
        let root = workbench::root();
        assert_eq!(execution_program(&root, doc.clone(), Some(&json!({"length": 8}))).unwrap().inputs["length"], 8.0);
        assert_eq!(execution_program(&root, doc.clone(), None).unwrap().inputs["length"], 20.0);
        assert!(execution_program(&root, doc, Some(&json!({"unknown": 2}))).is_err());
    }

    #[test]
    fn execution_waits_for_a_complete_session_aligned_period() {
        let mut series = ExecutionBars::new(5).unwrap();
        let minute = |minute: u8, price: f64| Candle {
            datetime: format!("2026-11-24T08:{minute:02}:00"),
            open: price, high: price + 1.0, low: price - 1.0, close: price, volume: 2.0,
            pl_date: 1261124, pl_time: 800 + u32::from(minute), source_rows: 1,
        };
        for i in 46..50 {
            assert!(series.push(minute(i, f64::from(i)), true).unwrap().is_none());
        }
        let bar = series.push(minute(50, 50.0), true).unwrap().unwrap();
        assert_eq!((bar.open, bar.high, bar.low, bar.close, bar.volume, bar.pl_time),
            (46.0, 51.0, 45.0, 50.0, 10.0, 850));
        assert!(series.push(minute(51, 51.0), true).unwrap().is_none());
        assert!(series.push(minute(53, 53.0), true).is_err());
    }

    #[test]
    fn enabled_strategy_waits_for_source_and_warmup_and_can_stop() {
        let path = workbench::root().join("outputs").join(library::fresh_id()).with_extension("sqlite");
        let stop = AtomicBool::new(false);
        let cancelled = wait_for_quotes(&path, "", "TXFL6", 2, 1, &stop, &|notice| {
            assert!(notice.starts_with("已啟用，等待"));
            stop.store(true, Ordering::Release);
        }, &|| false);
        assert!(matches!(cancelled, Err(error) if error == STOP));
        stop.store(false, Ordering::Release);
        let db = Connection::open(&path).unwrap();
        db.execute_batch(r#"
            CREATE TABLE live_sources(symbol,provider,connection,timezone,session_policy,minute_id);
            CREATE TABLE live_events(id INTEGER PRIMARY KEY,provider,connection,body);
            CREATE TABLE records(dataset_id,ordinal,datetime,payload);
            INSERT INTO live_sources VALUES('TX-DEC','kgi_quotecom','q','Asia/Taipei','taifex_regular',1);
            INSERT INTO live_events(provider,connection,body) VALUES('kgi_quotecom','q','{"instrument":"TX-DEC","provider_symbol":"TXFL6"}');
        "#).unwrap();
        let insert = |ordinal: i64, time: &str| {
            let payload = json!(crate::database::Payload::Bar {
                open:"100".into(), high:"100".into(), low:"100".into(), close:"100".into(), volume:"1".into()
            }).to_string();
            db.execute("INSERT INTO records VALUES(1,?1,?2,?3)", params![ordinal,time,payload]).unwrap();
        };
        insert(1, "2026-11-23T08:46:00");
        let quotes = wait_for_quotes(&path, "", "TXFL6", 2, 1, &stop, &|notice| {
            assert!(notice.contains("1/2"));
            insert(2, "2026-11-23T08:47:00");
        }, &|| false).unwrap().unwrap();
        assert_eq!(quotes.read(None, 2).unwrap().len(), 2);
        drop(quotes);
        drop(db);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn front_month_rolls_at_settlement_and_requires_unique_quote_mapping() {
        let dates = json!({"202602":"2026-02-23"});
        let day = chrono::NaiveDate::from_ymd_opt(2026, 11, 18).unwrap();
        assert_eq!(
            near_month("TXF", day.and_hms_opt(13, 29, 59).unwrap(), &dates),
            Some(("202611".into(), "TXFK6".into(), false))
        );
        for time in [(13, 30, 0), (14, 59, 59)] {
            assert_eq!(
                near_month("TXF", day.and_hms_opt(time.0,time.1,time.2).unwrap(), &dates),
                Some(("202611".into(), "TXFK6".into(), true))
            );
        }
        let now = day.and_hms_opt(15, 0, 0).unwrap();
        assert_eq!(
            near_month("TXF", now, &dates),
            Some(("202612".into(), "TXFL6".into(), false))
        );
        assert!(near_month("225M", now, &dates).is_none());
        assert_eq!(
            near_month("MXF", now, &dates),
            Some(("202612".into(), "MXFL6".into(), false))
        );
        assert_eq!(
            near_month("TMF", now, &dates),
            Some(("202612".into(), "TMFL6".into(), false))
        );
        let roll = chrono::NaiveDate::from_ymd_opt(2026, 12, 16)
            .unwrap()
            .and_hms_opt(15, 0, 0)
            .unwrap();
        assert_eq!(
            near_month("TXF", roll, &dates),
            Some(("202701".into(), "TXFA7".into(), false))
        );
        let feb = |d| chrono::NaiveDate::from_ymd_opt(2026,2,d).unwrap().and_hms_opt(15,0,0).unwrap();
        assert_eq!(near_month("TXF", feb(18), &dates).unwrap().0, "202602");
        assert_eq!(near_month("TXF", feb(23), &dates).unwrap().0, "202603");
        let path = workbench::root()
            .join("outputs")
            .join(library::fresh_id())
            .with_extension("sqlite");
        let db = Connection::open(&path).unwrap();
        db.execute_batch("CREATE TABLE live_sources(symbol,provider,connection,timezone,session_policy);
            CREATE TABLE live_events(id INTEGER PRIMARY KEY,provider,connection,body);
            INSERT INTO live_sources VALUES('TX-NOV','kgi_quotecom','q','Asia/Taipei','taifex_regular'),
            ('TX-DEC','kgi_quotecom','q','Asia/Taipei','taifex_regular'),('DECOY','kgi_quotecom','other','Asia/Taipei','taifex_regular');
            INSERT INTO live_events(provider,connection,body) VALUES('kgi_quotecom','q','{\"instrument\":\"TX-NOV\",\"provider_symbol\":\"TXFK6\"}'),
            ('kgi_quotecom','q','{\"instrument\":\"TX-DEC\",\"provider_symbol\":\"TXFL6\"}'),
            ('kgi_quotecom','q','{\"instrument\":\"DECOY\",\"provider_symbol\":\"TXFL6\"}');").unwrap();
        assert_eq!(
            front_contract(&path, "TXF", now, &dates).unwrap()["instrument"],
            "TX-DEC"
        );
        assert_eq!(
            front_contract(&path, "TXF", roll, &dates).unwrap()["instrument"],
            ""
        );
        drop(db);
        fs::remove_file(path).unwrap();
    }

    #[derive(Default)]
    struct FixtureBroker {
        sent: Vec<Value>,
        events: Vec<Value>,
    }

    impl Broker for FixtureBroker {
        fn submit(&mut self, order: Value) -> Result<(), String> {
            self.events.push(
                json!({"type":"fill","data":{"id":order["id"],"symbol":order["expected_symbol"],
                "quantity":"1","remaining":"0","price":"22000"}}),
            );
            self.sent.push(order);
            Ok(())
        }
        fn poll(&mut self) -> Result<(Vec<Value>, bool), String> {
            Ok((std::mem::take(&mut self.events), true))
        }
    }

    #[test]
    fn one_lot_order_waits_for_broker_fill_and_stopped_strategy_sends_nothing() {
        let mut broker = FixtureBroker::default();
        let stop = AtomicBool::new(false);
        assert_eq!(
            send_order(
                &mut broker,
                &stop,
                Action::Buy,
                true,
                "TXF",
                "202612",
                "TXFZ6"
            ),
            Ok(22000.0)
        );
        assert_eq!(broker.sent[0]["time_in_force"], "ioc");
        assert_eq!(broker.sent[0]["position_effect"], "open");
        assert_eq!(
            send_order(
                &mut broker,
                &stop,
                Action::Sell,
                false,
                "TXF",
                "202612",
                "TXFZ6"
            ),
            Ok(22000.0)
        );
        assert_eq!(broker.sent[1]["position_effect"], "close");
        stop.store(true, Ordering::Release);
        assert!(send_order(
            &mut broker,
            &stop,
            Action::SellShort,
            true,
            "TXF",
            "202612",
            "TXFZ6"
        )
        .is_err());
        assert_eq!(broker.sent.len(), 2);
    }

    #[test]
    fn unsupported_reversal_or_multiple_signals_cannot_generate_orders() {
        let buy = Order {
            total: false,
            action: Action::Buy,
            label: "fixture".into(),
            quantity: Some(1),
            price: None,
            kind: Kind::Market,
        };
        assert_eq!(plan(&[buy.clone()], 0), Ok(Some((Action::Buy, true))));
        assert_eq!(plan(&[buy.clone()], 1), Ok(None));
        assert!(plan(&[buy.clone()], -1).is_err());
        assert!(plan(&[buy.clone(), buy], 0).is_err());
    }
}
