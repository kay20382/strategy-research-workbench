//! Single-instrument, closed-bar event engine. Money uses fixed micro-units.
use crate::{
    database::{self, Payload},
    powerlanguage::{Action, ExecutionContext, Kind, Order, Program, Runtime},
};
use chrono::{Datelike, NaiveDate, NaiveDateTime, Timelike};
use rusqlite::params;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, path::Path};

const SCALE: f64 = 1_000_000.0;
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum IntrabarPolicy {
    Adverse,
    #[default]
    NearestExtreme,
}
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum PriceRounding {
    #[default]
    McNative,
    /// 歷史研究重現模式，不含 MC 原生近整數靠攏。
    Directional,
    /// 本機 MC 原生近整數容差；目前僅驗證一點跳動、非負委託價。
    McNativeOnePoint,
    Nearest,
    BuyCeilSellFloor,
    BuyFloorSellCeil,
    Ceil,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Session {
    pub name: String,
    pub start_minute: u32,
    pub end_minute: u32,
}
fn money(x: f64) -> Result<i128, String> {
    if !x.is_finite() || x.abs() > 1e15 {
        return Err("金額非有限或超過可支援範圍".into());
    }
    Ok((x * SCALE).round() as i128)
}
fn amount(x: i128) -> f64 {
    x as f64 / SCALE
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// 目前驗證模式：disabled或any_signal；different_signal尚待另一組原生驗證。
    #[serde(default = "disabled_pyramiding")]
    pub pyramiding: String,
    #[serde(default)]
    pub max_position_contracts: Option<i64>,
    #[serde(default)]
    pub max_entry_batches: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interval_seconds: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interval_days: Option<u32>,
    /// 由本版原生建構子證明為 Simple 的變數；歷史回呼中固定讀取當棒值。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub simple_variables: Vec<String>,
    /// PLA原始簽章：16=Simple、32=Series。缺省保留既有Series處理，不猜測。
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub function_storage_types: BTreeMap<String, u8>,
    /// Explicit source candle interval; None preserves the one-minute legacy input.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_interval_minutes: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tick_dataset_id: Option<i64>,
    #[serde(default)]
    pub build_volume_on: Option<String>,
    #[serde(default)]
    pub strategy_name: String,
    #[serde(default)]
    pub price_rounding: PriceRounding,
    /// MC 商品 PriceScale；unit=1/PriceScale，與 MinMove 分開保存。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub price_scale: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub minimum_move: Option<u64>,
    /// MultiCharts 內部候選委託選擇旗標；0/1 兩種表均已由 DLL 靜態還原，
    /// 旗標如何由宿主狀態決定仍未知。None 保留舊撮合方式。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mc_order_priority_mode: Option<u8>,
    /// OpenEntryMaxProfit 是否回傳已扣進場佣金的最大浮動獲利；缺省採 KAIv7 對照相符的 MC 語意。
    /// 設為 false 僅供舊 gross 語意差異診斷。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mc_open_entry_max_profit_net: Option<bool>,
    #[serde(default)]
    pub intrabar_policy: IntrabarPolicy,
    #[serde(default)]
    pub sessions: Vec<Session>,
    pub dataset_id: i64,
    pub from: String,
    pub to: String,
    pub interval_minutes: u32,
    pub bar_anchor_minutes: u32,
    pub timestamp_role: String,
    pub warmup_bars: usize,
    pub initial_capital: f64,
    pub tick_size: f64,
    pub point_value: f64,
    pub commission_per_side: f64,
    pub slippage_ticks: u32,
    pub contracts: i64,
    pub margin_per_contract: Option<f64>,
    pub enforce_cash_balance: bool,
    pub inputs: BTreeMap<String, f64>,
    pub functions: BTreeMap<String, String>,
    pub research_note: String,
}
fn disabled_pyramiding() -> String { "disabled".into() }
impl Config {
    fn native_price_grid(&self) -> Result<(f64,u64),String> {
        let (unit,step)=match (self.price_scale,self.minimum_move) {
            (Some(scale),Some(step)) if scale.is_finite() && scale>0.0 =>(1.0/scale,step),
            _=>return Err("MC 原生取整須提供商品 price_scale 與 minimum_move，不能僅由 tick_size 反推價格刻度".into()),
        };
        if !unit.is_finite() || unit<=0.0 || step==0 || step>1_000_000_000
            || (unit*step as f64-self.tick_size).abs()>self.tick_size*1e-12 {
            return Err("商品價格刻度與最小跳動不符：minimum_move ÷ price_scale 必須等於 tick_size".into());
        }
        Ok((unit,step))
    }
    pub fn validate(&self) -> Result<(), String> {
        if !["disabled", "any_signal"].contains(&self.pyramiding.as_str()) {
            return Err("加碼模式目前僅支援disabled／any_signal；排除同訊號模式尚待原生驗證".into());
        }
        if self.max_position_contracts.is_some_and(|v|v<1||v>1_000_000) || self.max_entry_batches.is_some_and(|v|v<1||v>1_000_000) {
            return Err("持倉與進場批次上限須為1至1000000".into());
        }
        if self.pyramiding=="any_signal" && (self.max_position_contracts.is_none() || self.max_entry_batches.is_none()) {
            return Err("開啟加碼須明示總持倉口數及同方向進場批次上限".into());
        }
        if self.interval_seconds.is_some_and(|n|n==0||n>86400) || self.interval_days.is_some_and(|n|n==0||n>365) || (self.interval_seconds.is_some()&&self.interval_days.is_some()) || ((self.interval_seconds.is_some()||self.interval_days.is_some())&&self.interval_minutes!=0) {
            return Err("秒K／日K週期無效，且不可同時指定不同週期單位".into());
        }
        if self.source_interval_minutes.is_some_and(|n| n==0 || self.interval_minutes==0 || self.interval_minutes % n != 0) {
            return Err("來源 K 棒週期須為正整數分鐘，且須整除目標週期".into());
        }
        if self
            .build_volume_on
            .as_ref()
            .is_some_and(|s| s != "trade_volume")
        {
            return Err(
                "目前 Build Volume On 僅支援明示 trade_volume；成交筆數資料尚未接入".into(),
            );
        }
        let mut occupied = std::collections::HashSet::new();
        for session in &self.sessions {
            if session.start_minute >= 1440
                || session.end_minute >= 1440
                || session.start_minute == session.end_minute
            {
                return Err("交易時段須為 0 至 1439 分鐘，起訖不可相同".into());
            }
            let duration = (session.end_minute + 1440 - session.start_minute) % 1440;
            for offset in 1..=duration {
                let minute = (session.start_minute + offset) % 1440;
                if !occupied.insert(minute) {
                    return Err("交易時段重疊".into());
                }
            }
        }
        if self.initial_capital <= 0.0
            || !self.initial_capital.is_finite()
            || self.tick_size <= 0.0
            || !self.tick_size.is_finite()
            || self.point_value <= 0.0
            || !self.point_value.is_finite()
            || self.tick_size > 1e6
            || self.point_value > 1e6
        {
            return Err("資本、跳動點與每點價值須為有效正值".into());
        }
        money(self.initial_capital)?;
        if self.price_rounding==PriceRounding::McNative {self.native_price_grid()?;}
        if self.price_rounding == PriceRounding::McNativeOnePoint && self.tick_size != 1.0 {
            return Err("MC 原生取整目前僅驗證最小跳動為 1 點的商品".into());
        }
        let tv = money(self.tick_size * self.point_value)?;
        if tv == 0 || tv > 1_000_000_000_000 {
            return Err("每跳價值超過第一版固定精度支援範圍".into());
        }
        if self.commission_per_side < 0.0 || !self.commission_per_side.is_finite() {
            return Err("單邊成本不可為負".into());
        }
        money(self.commission_per_side)?;
        if self
            .margin_per_contract
            .is_some_and(|m| !m.is_finite() || m <= 0.0)
        {
            return Err("保證金須為正值或 null（未建模）".into());
        }
        if let Some(m) = self.margin_per_contract {
            money(m)?;
        }
        if !(1..=1_000_000).contains(&self.contracts)
            || self.warmup_bars > 1_000_000
            || self.slippage_ticks > 100_000
        {
            return Err("數量、暖機或滑價超出範圍".into());
        }
        if self.interval_minutes > 1440
            || self.bar_anchor_minutes >= 1440
            || !["start", "end"].contains(&self.timestamp_role.as_str())
        {
            return Err("時間標記須明確為 start／end；週期與對齊點須在一天內".into());
        }
        if self.mc_order_priority_mode.is_some_and(|mode| mode > 1) {
            return Err("MC 候選委託優先模式只接受 0、1，或 null 使用原撮合方式".into());
        }
        if !self.from.is_empty() {parse_dt(&self.from)?;}
        if !self.to.is_empty() {parse_dt(&self.to)?;}
        if !self.from.is_empty() && !self.to.is_empty() && parse_dt(&self.from)? > parse_dt(&self.to)? {
            return Err("開始時間晚於結束時間".into());
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Candle {
    pub datetime: String,
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
    pub volume: f64,
    pub pl_date: i32,
    pub pl_time: u32,
    pub source_rows: usize,
}
pub(crate) fn candle(datetime: String, payload: Payload) -> Result<Candle, String> {
    let Payload::Bar {
        open,
        high,
        low,
        close,
        volume,
    } = payload
    else {
        return Err("回測第一版僅支援資料庫 minute／daily K 棒".into());
    };
    let dt = parse_dt(&datetime)?;
    let num = |s: String| -> Result<f64, String> {
        let n = s.parse::<f64>().map_err(|_| "行情不是數值")?;
        if n.is_finite() {
            Ok(n)
        } else {
            Err("行情含非有限值".into())
        }
    };
    let b = Candle {
        datetime,
        open: num(open)?,
        high: num(high)?,
        low: num(low)?,
        close: num(close)?,
        volume: num(volume)?,
        pl_date: (dt.year() - 1900) * 10000 + dt.month() as i32 * 100 + dt.day() as i32,
        pl_time: dt.hour() * 100 + dt.minute(),
        source_rows: 1,
    };
    if b.high < b.low
        || b.high < b.open.max(b.close)
        || b.low > b.open.min(b.close)
        || b.volume < 0.0
    {
        return Err("資料庫 OHLCV 無效".into());
    }
    Ok(b)
}
fn parse_dt(s: &str) -> Result<NaiveDateTime, String> {
    if s.len() == 10 {
        return NaiveDate::parse_from_str(s, "%Y-%m-%d")
            .map(|d| d.and_hms_opt(0, 0, 0).unwrap())
            .map_err(|e| e.to_string());
    }
    NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S%.f").or_else(|_|NaiveDateTime::parse_from_str(s,"%Y/%m/%d %H:%M:%S%.f")).map_err(|e| e.to_string())
}
fn database_cutoff(value:&str)->Result<String,String> {
    if value.is_empty() {Ok("z".into())}
    else {Ok(parse_dt(value)?.format("%Y-%m-%dT%H:%M:%S%.9f").to_string())}
}

fn session_bucket(dt: NaiveDateTime, cfg: &Config) -> Result<Option<i64>, String> {
    let minute = dt.hour() * 60 + dt.minute();
    let m = dt.and_utc().timestamp() / 60;
    let midnight = m - minute as i64;
    for s in &cfg.sessions {
        let duration = (s.end_minute + 1440 - s.start_minute) % 1440;
        let elapsed = (minute + 1440 - s.start_minute) % 1440;
        let inside = if cfg.timestamp_role == "end" {
            elapsed > 0 && elapsed <= duration
        } else {
            elapsed < duration
        };
        if !inside {
            continue;
        }
        let mut start = midnight + s.start_minute as i64;
        if minute < s.start_minute {
            start -= 1440;
        }
        let n = cfg.interval_minutes.max(1) as i64;
        let elapsed = m - start;
        let offset = if cfg.timestamp_role == "end" {
            ((elapsed - 1).div_euclid(n) + 1) * n
        } else {
            elapsed.div_euclid(n) * n
        };
        return Ok(Some(start + offset.min(duration as i64)));
    }
    Ok(None)
}
#[derive(Clone, Debug, Serialize)]
pub struct Provenance {
    pub outside_session_rows: usize,
    pub dataset_id: i64,
    pub symbol: String,
    pub source: String,
    pub kind: String,
    pub timezone: String,
    pub source_timestamp_role: String,
    pub source_sha256: String,
    pub bars_sha256: String,
    pub source_rows: usize,
    pub partial_bars: usize,
}
pub fn load(database: &Path, cfg: &Config) -> Result<(Vec<Candle>, Provenance), String> {
    cfg.validate()?;
    if cfg.interval_seconds.is_some(){return load_tick_candles(database,cfg);}
    let conn = database::open_existing(database)?;
    conn.execute_batch("BEGIN DEFERRED TRANSACTION")
        .map_err(|e| e.to_string())?;
    let (symbol, source, kind, timezone, role, sha): (
        String,
        String,
        String,
        String,
        String,
        String,
    ) = conn
        .query_row(
            "SELECT symbol,source,kind,timezone,timestamp_role,sha256 FROM datasets WHERE id=?1",
            [cfg.dataset_id],
            |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                ))
            },
        )
        .map_err(|e| e.to_string())?;
    if !["minute", "daily"].contains(&kind.as_str()) {
        return Err("目前僅執行 minute／daily 資料，逐筆成交、十檔及自訂資料執行器尚未完成".into());
    }
    if role != "unspecified" && role != cfg.timestamp_role {
        return Err("任務與資料集的時間標記意義不一致".into());
    }
    if kind == "daily" && cfg.interval_minutes != 0 {
        return Err("日線不能還原分鐘線；日線請設定 interval_minutes=0".into());
    }
    let mut stmt=conn.prepare("SELECT datetime,available_at,payload FROM records WHERE dataset_id=?1 AND datetime>=?2 AND datetime<=?3 ORDER BY datetime,ordinal").map_err(|e|e.to_string())?;
    // 儲存表時間固定九位小數秒，截止格式不同不應漏掉相同時間的最後一棒。
    let to = database_cutoff(&cfg.to)?;
    // Load preceding observations as well: the trading start must not consume its own warmup.
    let rows = stmt
        .query_map(params![cfg.dataset_id, "", to], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, Option<String>>(1)?,
                r.get::<_, String>(2)?,
            ))
        })
        .map_err(|e| e.to_string())?;
    let mut bars: Vec<Candle> = vec![];
    let mut key: i64 = i64::MIN;
    let mut previous = String::new();
    let mut count = 0;
    let mut outside_session_rows = 0;
    for row in rows {
        let (datetime, available, payload) = row.map_err(|e| e.to_string())?;
        if !previous.is_empty() && datetime <= previous {
            return Err("資料庫有重複或不遞增的 K 棒".into());
        }
        previous = datetime.clone();
        if available.as_ref().is_some_and(|v| v > &datetime) {
            return Err("行情可用時間晚於 K 棒標記；此版本尚不能排程延遲發布資料".into());
        }
        let mut b = candle(
            datetime,
            serde_json::from_str(&payload).map_err(|e| e.to_string())?,
        )?;
        count += 1;
        let next_key = if kind=="daily" && cfg.interval_days.unwrap_or(1)>1 {
            (count as i64-1)/cfg.interval_days.unwrap() as i64
        } else if kind == "minute" && !cfg.sessions.is_empty() {
            match session_bucket(parse_dt(&b.datetime)?, cfg)? {
                Some(k) => k,
                None => {
                    outside_session_rows += 1;
                    continue;
                }
            }
        } else if kind == "minute" && cfg.interval_minutes > 1 {
            let m = parse_dt(&b.datetime)?.and_utc().timestamp() / 60;
            let a = cfg.bar_anchor_minutes as i64;
            let n = cfg.interval_minutes as i64;
            let bucket = if cfg.timestamp_role == "end" {
                (m - a - 1).div_euclid(n) + 1
            } else {
                (m - a).div_euclid(n)
            };
            bucket * n + a
        } else {
            count as i64
        };
        if next_key == key {
            let current = bars.last_mut().unwrap();
            current.high = current.high.max(b.high);
            current.low = current.low.min(b.low);
            current.close = b.close;
            current.volume += b.volume;
            current.source_rows += 1;
            if kind=="daily"{current.datetime=b.datetime;current.pl_date=b.pl_date;current.pl_time=b.pl_time;}
        } else {
            if kind == "minute" && cfg.interval_minutes > 1 {
                let label = chrono::DateTime::from_timestamp(next_key * 60, 0)
                    .ok_or("彙整時間超出範圍")?
                    .naive_utc();
                b.datetime = label.format("%Y-%m-%dT%H:%M:%S").to_string();
                b.pl_date =
                    (label.year() - 1900) * 10000 + label.month() as i32 * 100 + label.day() as i32;
                b.pl_time = label.hour() * 100 + label.minute();
            }
            bars.push(b);
            key = next_key;
        }
    }
    if !cfg.from.is_empty() {
        let from = parse_dt(&cfg.from)?;
        let first = bars
            .iter()
            .position(|b| parse_dt(&b.datetime).is_ok_and(|t| t >= from))
            .ok_or("指定期間沒有行情")?;
        if first < cfg.warmup_bars {
            return Err("開始日前的暖機行情不足；請調整開始日或暖機設定".into());
        }
        bars.drain(..first - cfg.warmup_bars);
    }
    if bars.len() <= cfg.warmup_bars {
        return Err("資料不足以完成指定暖機".into());
    }
    for b in &bars {
        for p in [b.open, b.high, b.low, b.close] {
            ticks(p, cfg.tick_size, true)?;
        }
    }
    let partial = if kind=="daily"{bars.iter().filter(|b|b.source_rows!=cfg.interval_days.unwrap_or(1) as usize).count()}else if kind == "minute" && cfg.interval_minutes > 1 {
        bars.iter()
            .filter(|b| b.source_rows != (cfg.interval_minutes / cfg.source_interval_minutes.unwrap_or(1)) as usize)
            .count()
    } else {
        0
    };
    let bars_sha256 = format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&bars).map_err(|e| e.to_string())?)
    );
    Ok((
        bars,
        Provenance {
            outside_session_rows,
            dataset_id: cfg.dataset_id,
            symbol,
            source,
            kind,
            timezone,
            source_timestamp_role: role,
            source_sha256: sha,
            bars_sha256,
            source_rows: count,
            partial_bars: partial,
        },
    ))
}
fn load_tick_candles(database:&Path,cfg:&Config)->Result<(Vec<Candle>,Provenance),String>{
    use crate::{tick_bars::{Aggregator,TickBar},tick_store};
    let seconds=cfg.interval_seconds.ok_or("缺少秒K週期")?;
    if cfg.timestamp_role!="end"{return Err("秒K目前使用結束時間標記 end".into());}
    let conn=database::open_existing(database)?;conn.execute_batch("BEGIN DEFERRED TRANSACTION").map_err(|e|e.to_string())?;
    let (symbol,source,kind,timezone,role,sha,first):(String,String,String,String,String,String,String)=conn.query_row("SELECT symbol,source,kind,timezone,timestamp_role,sha256,first_datetime FROM datasets WHERE id=?1",[cfg.dataset_id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?,r.get(6)?))).map_err(|e|e.to_string())?;
    if kind!="trade"||!tick_store::contains(&conn,cfg.dataset_id)?{return Err("秒K只接受標準 Tick 資料源".into());}
    let session_mode=!cfg.sessions.is_empty();
    if session_mode{let mut s=cfg.sessions.iter().map(|s|(s.start_minute,s.end_minute)).collect::<Vec<_>>();s.sort();if s!=vec![(525,825),(900,300)]{return Err("秒K交易時段目前只已實作臺指日夜盤；其他商品請使用全天對齊或另提供時段規則".into());}}
    let earliest=parse_dt(&first)?;let requested=if cfg.from.is_empty(){None}else{Some(parse_dt(&cfg.from)?)};
    let cutoff=if cfg.to.is_empty(){None}else{Some(parse_dt(&cfg.to)?)};
    let mut span=(seconds as i64*(cfg.warmup_bars as i64+2)*2).max(6*3600);
    loop{
        let lower=requested.map(|d|(d-chrono::Duration::seconds(span)).max(earliest)).unwrap_or(earliest);
        let mut agg=if session_mode{Aggregator::with_sessions_close_exclusive(seconds as i64)?}else{Aggregator::new(seconds as i64,cfg.bar_anchor_minutes as i64*60)?};
        let mut bars=vec![];let mut outside=0;
        let add=|bars:&mut Vec<Candle>,b:TickBar|->Result<(),String>{
            let mut c=candle(b.datetime,Payload::Bar{open:b.open,high:b.high,low:b.low,close:b.close,volume:b.volume})?;c.source_rows=b.tick_count as usize;
            if cutoff.is_none_or(|to|parse_dt(&c.datetime).is_ok_and(|d|d<=to)){bars.push(c);}Ok(())
        };
        let count=tick_store::visit(&conn,cfg.dataset_id,&lower.format("%Y-%m-%dT%H:%M:%S%.9f").to_string(),&cfg.to,|ns,price,volume|{
            let dt=chrono::DateTime::from_timestamp(ns.div_euclid(1_000_000_000),ns.rem_euclid(1_000_000_000) as u32).ok_or("Tick時間無效")?.naive_utc();
            if agg.window(dt).is_none(){outside+=1;return Ok(());}
            if let Some(bar)=agg.push_fixed(dt,price as i128,volume as i128)?{add(&mut bars,bar)?;}Ok(())
        })?;
        if let Some(bar)=agg.finish()?{add(&mut bars,bar)?;}
        if let Some(from)=requested{
            let at=bars.iter().position(|b|parse_dt(&b.datetime).is_ok_and(|d|d>=from)).ok_or("指定期間沒有Tick可彙整成完整秒K")?;
            if at<cfg.warmup_bars+usize::from(lower>earliest){
                if lower>earliest{span=span.checked_mul(2).ok_or("暖機區間溢位")?;continue;}
                return Err("開始日前的 Tick 暖機行情不足".into());
            }
            bars.drain(..at-cfg.warmup_bars);
        }
        if bars.len()<=cfg.warmup_bars{return Err("Tick彙整結果不足以完成暖機".into());}
        for b in &bars{for p in [b.open,b.high,b.low,b.close]{ticks(p,cfg.tick_size,true)?;}}
        let bars_sha256=format!("{:x}",Sha256::digest(serde_json::to_vec(&bars).map_err(|e|e.to_string())?));
        return Ok((bars,Provenance{outside_session_rows:outside,dataset_id:cfg.dataset_id,symbol,source,kind,timezone,source_timestamp_role:role,source_sha256:sha,bars_sha256,source_rows:count,partial_bars:0}));
    }
}
fn ticks(p: f64, size: f64, strict: bool) -> Result<i64, String> {
    let x = p / size;
    if !x.is_finite() || x.abs() > 1e9 {
        return Err("價格超出支援範圍".into());
    }
    if strict && (x - x.round()).abs() > 1e-6 {
        return Err(format!("行情價格 {p} 不符合最小跳動 {size}"));
    }
    Ok(x.round() as i64)
}

#[derive(Clone, Debug, Serialize)]
pub struct Fill {
    pub is_entry: bool,
    pub order_id: usize,
    pub bar: usize,
    pub datetime: String,
    pub phase: String,
    pub action: String,
    pub label: String,
    pub side: i64,
    pub quantity: i64,
    pub price: f64,
    pub fee: f64,
}
#[derive(Clone, Debug, Serialize)]
pub struct Trade {
    pub id: usize,
    pub entry_order_id: usize,
    pub exit_order_id: usize,
    pub side: i64,
    pub quantity: i64,
    pub entry_bar: usize,
    pub exit_bar: usize,
    pub entry_datetime: String,
    pub exit_datetime: String,
    pub entry_label: String,
    pub exit_label: String,
    pub entry_price: f64,
    pub exit_price: f64,
    pub gross: f64,
    pub costs: f64,
    pub net: f64,
    pub mfe: f64,
    pub mae: f64,
    pub mfe_gross: f64,
    pub mae_gross: f64,
    pub cumulative: f64,
}
#[derive(Clone, Debug, Serialize)]
pub struct Event {
    pub order_id: usize,
    pub bar: usize,
    pub label: String,
    pub status: String,
    pub reason: String,
    pub raw_order_price: Option<f64>,
}
#[derive(Clone, Debug, Serialize)]
pub struct Equity {
    pub datetime: String,
    pub equity: f64,
    pub cash: f64,
    pub position: i64,
    pub drawdown: f64,
    pub equity_low: f64,
    pub equity_high: f64,
    pub equity_low_after_high: f64,
    pub bar_drawdown: f64,
    pub gross_equity_low: f64,
    pub gross_equity_close: f64,
    pub position_equity_low: Option<f64>,
}
#[derive(Clone, Debug)]
struct Position {
    order_id: usize,
    side: i64,
    quantity: i64,
    price: i64,
    bar: usize,
    label: String,
    datetime: String,
    entry_fee: i128,
    mfe: i128,
    mae: i128,
}
#[derive(Clone)]
struct Account {
    cash: i128,
    positions: Vec<Position>,
    peak: i128,
    max_drawdown: i128,
    max_drawdown_pct: f64,
    trade_count: usize,
    closed_net: i128,
    bar_equity_low: i128,
    bar_equity_high: i128,
    bar_equity_low_after_high: i128,
    bar_drawdown: i128,
    paid_fees: i128,
    bar_gross_equity_low: i128,
    bar_position_equity_low: i128,
}
impl Account {
    fn quantity(&self) -> i64 { self.positions.iter().map(|p|p.quantity).sum() }
    fn equity(&self, p: i64, tv: i128) -> i128 {
        self.cash + self.positions.iter().map(|x|
            (p-x.price) as i128 * x.side as i128 * x.quantity as i128 * tv).sum::<i128>()
    }
    fn mark(&mut self, p: i64, tv: i128) {
        for pos in &mut self.positions {
            let pnl = (p - pos.price) as i128 * pos.side as i128 * pos.quantity as i128 * tv;
            pos.mfe = pos.mfe.max(pnl);
            pos.mae = pos.mae.min(pnl);
        }
        let equity = self.equity(p, tv);
        self.bar_equity_low = self.bar_equity_low.min(equity);
        if equity >= self.bar_equity_high {
            self.bar_equity_high = equity;
            self.bar_equity_low_after_high = equity;
        } else {
            self.bar_equity_low_after_high = self.bar_equity_low_after_high.min(equity);
        }
        self.bar_gross_equity_low = self.bar_gross_equity_low.min(equity + self.paid_fees);
        if !self.positions.is_empty() {
            self.bar_position_equity_low = self.bar_position_equity_low.min(equity);
        }
        self.peak = self.peak.max(equity);
        let dd = equity - self.peak;
        self.bar_drawdown = self.bar_drawdown.min(dd);
        self.max_drawdown = self.max_drawdown.min(dd);
        if self.peak > 0 {
            self.max_drawdown_pct = self
                .max_drawdown_pct
                .max(-dd as f64 / self.peak as f64 * 100.0);
        }
    }
}
#[derive(Clone)]
struct Pending {
    id: usize,
    order: Order,
}
#[derive(Clone)]
struct Delta {
    account: Account,
    fills: Vec<Fill>,
    trades: Vec<Trade>,
    events: Vec<Event>,
}
fn buy_side(a: Action) -> bool {
    matches!(a, Action::Buy | Action::BuyToCover)
}
fn active(o: &Order, a: &Account, cfg: &Config) -> bool {
    match o.action {
        Action::Sell => a.positions.first().is_some_and(|p| p.side == 1),
        Action::BuyToCover => a.positions.first().is_some_and(|p| p.side == -1),
        Action::Buy => cfg.pyramiding=="any_signal" || !a.positions.first().is_some_and(|p| p.side == 1),
        Action::SellShort => cfg.pyramiding=="any_signal" || !a.positions.first().is_some_and(|p| p.side == -1),
    }
}
fn trigger(o: &Order, p: i64, cfg: &Config) -> Result<bool, String> {
    if matches!(o.kind, Kind::Market | Kind::Close) {
        return Ok(true);
    }
    let px = order_ticks(o, cfg)?;
    Ok(match o.kind {
        Kind::Limit => {
            if buy_side(o.action) {
                p <= px
            } else {
                p >= px
            }
        }
        Kind::Stop => {
            if buy_side(o.action) {
                p >= px
            } else {
                p <= px
            }
        }
        _ => true,
    })
}
fn order_ticks(o: &Order, cfg: &Config) -> Result<i64, String> {
    let raw = o.price.ok_or("限價／停損單缺少價格")? / cfg.tick_size;
    if !raw.is_finite() || raw.abs() > 1e9 {
        return Err("委託價格不合理".into());
    }
    if matches!(cfg.price_rounding,PriceRounding::McNative|PriceRounding::McNativeOnePoint) {
        let (unit,step)=if cfg.price_rounding==PriceRounding::McNative {cfg.native_price_grid()?} else {(1.0,1)};
        if cfg.price_rounding==PriceRounding::McNativeOnePoint && (cfg.tick_size!=1.0 || raw<0.0) {
            return Err("舊版一點取整模式僅接受一點跳動及非負委託價".into());
        }
        let up=!matches!((o.kind,buy_side(o.action)),(Kind::Limit,true)|(Kind::Stop,false));
        let units=mc_native_price_units(o.price.unwrap(),unit,step,up)?;
        if units % step as i64 !=0 {
            return Err("MC 原生價格函式傳回非最小跳動網格值，目前整跳撮合不能表示此結果".into());
        }
        return Ok(units/step as i64);
    }
    if cfg.price_rounding == PriceRounding::Nearest {
        return Ok(raw.round() as i64);
    }
    if cfg.price_rounding == PriceRounding::Ceil {
        return Ok(raw.ceil() as i64);
    }
    if cfg.price_rounding == PriceRounding::BuyCeilSellFloor {
        return Ok(if buy_side(o.action) {raw.ceil() as i64} else {raw.floor() as i64});
    }
    if cfg.price_rounding == PriceRounding::BuyFloorSellCeil {
        return Ok(if buy_side(o.action) {raw.floor() as i64} else {raw.ceil() as i64});
    }
    Ok(match (o.kind, buy_side(o.action)) {
        (Kind::Limit, true) | (Kind::Stop, false) => raw.floor() as i64,
        _ => raw.ceil() as i64,
    })
}

/// StrategyExecutor.dll RVA 0x2cd20 的精確數值流程；9,432 組原 DLL 實測向量。
/// unit 與 step 不可合併為 tick_size，亦不可用 tick_size*0.01 代替原生容差。
fn mc_native_price_units(price:f64,unit:f64,step:u64,up:bool)->Result<i64,String> {
    if !price.is_finite() || !unit.is_finite() || unit<=0.0 || step==0 || step>1_000_000_000 {
        return Err("MC 原生價格刻度無效".into());
    }
    let q=price/unit;
    if !q.is_finite() || q.abs()>9_000_000_000_000_000.0 {return Err("MC 原生價格超出已驗證整數範圍".into());}
    let magnitude=q.abs();let lower=magnitude.floor();let fraction=magnitude-lower;
    let tolerance=unit*0.01;
    let snapped=if fraction<=tolerance {lower} else if fraction>=1.0-tolerance {lower+1.0} else {magnitude};
    let signed=if q<0.0 {-snapped} else {snapped};
    let quotient=(signed/step as f64).trunc() as i64;
    let multiple=quotient.checked_mul(step as i64).ok_or("MC 原生價格乘法超界")?;
    // 原 DLL 將負整數轉成 unsigned double 後比較；保留其已實測分支。
    let comparison=if multiple<0 {multiple as f64+18_446_744_073_709_551_616.0} else {multiple as f64};
    if comparison<signed {
        if up {quotient.checked_add(1).and_then(|v|v.checked_mul(step as i64)).ok_or("MC 原生價格進位超界".into())}
        else {Ok(multiple)}
    } else {Ok(signed.trunc() as i64)}
}

/// 將 PowerLanguage 動作轉成 MC EMCOrderType 的兩個已觀察欄位：
/// entry/exit=0/1、buy/sell=1/2。EMCOrderType 常數來自 StudyRunner TypeLib；
/// 將它們對應到 StrategyExecutor +0xe4/+0xe8 是根據欄位判斷與表格推導，
/// 尚未由同一筆執行期委託資料直接驗證，所以僅在明示的實驗模式啟用。
fn mc_order_class(action: Action) -> usize {
    match action {
        Action::Buy => 0,         // entry, buy
        Action::BuyToCover => 1,  // exit, buy
        Action::SellShort => 2,   // entry, sell
        Action::Sell => 3,        // exit, sell
    }
}

/// 由 MC DLL 的四張 byte 表逐項轉錄。position 是 MarketPosition(-1/0/1)。
fn mc_order_priority(action: Action, position: i64, mode: u8) -> Option<u8> {
    let class = mc_order_class(action);
    let table: [u8; 4] = match (position, mode) {
        (-1, 0) => [3, 0, 0, 2],
        (-1, 1) => [3, 0, 1, 2],
        (1, 0) => [0, 2, 3, 0],
        (1, 1) => [1, 2, 3, 0],
        _ => return None,
    };
    Some(table[class])
}
fn close_position(
    d: &mut Delta,
    pending: &Pending,
    price: i64,
    bar: usize,
    b: &Candle,
    phase: &str,
    cfg: &Config,
    tv: i128,
) -> Result<(), String> {
    let mut remaining = pending.order.quantity.unwrap_or(i64::MAX);
    let mut at = 0;
    while at < d.account.positions.len() {
        if pending.order.total && remaining == 0 { break; }
        let mut pos = d.account.positions.remove(at);
        let old_quantity = pos.quantity;
        let q = if pending.order.total { remaining.min(old_quantity) }
                else { pending.order.quantity.unwrap_or(old_quantity).min(old_quantity) };
        if q < old_quantity {
            let mut rest = pos.clone();
            rest.quantity = old_quantity-q;
            rest.entry_fee = pos.entry_fee * rest.quantity as i128 / old_quantity as i128;
            rest.mfe = pos.mfe * rest.quantity as i128 / old_quantity as i128;
            rest.mae = pos.mae * rest.quantity as i128 / old_quantity as i128;
            pos.entry_fee -= rest.entry_fee;
            pos.mfe -= rest.mfe;
            pos.mae -= rest.mae;
            d.account.positions.insert(at,rest);
            at += 1;
        }
        pos.quantity = q;
        if pending.order.total { remaining -= q; }
    let fee = money(cfg.commission_per_side)? * pos.quantity as i128;
    let gross = (price - pos.price) as i128 * pos.side as i128 * pos.quantity as i128 * tv;
    d.account.cash += gross - fee;
    d.account.paid_fees += fee;
    let net = gross - fee - pos.entry_fee;
    d.account.closed_net += net;
    d.account.trade_count += 1;
    d.fills.push(Fill {
        is_entry: false,
        order_id: pending.id,
        bar,
        datetime: b.datetime.clone(),
        phase: phase.into(),
        action: pending.order.action.name().into(),
        label: pending.order.label.clone(),
        side: -pos.side,
        quantity: pos.quantity,
        price: price as f64 * cfg.tick_size,
        fee: amount(fee),
    });
    d.trades.push(Trade {
        id: d.account.trade_count,
        entry_order_id: pos.order_id,
        exit_order_id: pending.id,
        side: pos.side,
        quantity: pos.quantity,
        entry_bar: pos.bar,
        exit_bar: bar,
        entry_datetime: pos.datetime,
        exit_datetime: b.datetime.clone(),
        entry_label: pos.label,
        exit_label: pending.order.label.clone(),
        entry_price: pos.price as f64 * cfg.tick_size,
        exit_price: price as f64 * cfg.tick_size,
        gross: amount(gross),
        costs: amount(fee + pos.entry_fee),
        net: amount(net),
        mfe: amount((pos.mfe - pos.entry_fee).max(0)),
        mae: amount(pos.mae - pos.entry_fee),
        mfe_gross: amount(pos.mfe),
        mae_gross: amount(pos.mae),
        cumulative: amount(d.account.closed_net),
    });
    }
    Ok(())
}
fn execute(
    d: &mut Delta,
    pending: &Pending,
    base: i64,
    bar: usize,
    b: &Candle,
    phase: &str,
    cfg: &Config,
    tv: i128,
) -> Result<(), String> {
    let o = &pending.order;
    let signed = if buy_side(o.action) { 1 } else { -1 };
    let price = base
        + if o.kind != Kind::Limit {
            signed * cfg.slippage_ticks as i64
        } else {
            0
        };
    let mut reason = String::new();
    match o.action {
        Action::Sell | Action::BuyToCover => {
            if active(o, &d.account, cfg) {
                d.account.mark(price, tv);
                close_position(d, pending, price, bar, b, phase, cfg, tv)?;
            } else {
                reason = "沒有可平倉的對應部位".into();
            }
        }
        Action::Buy | Action::SellShort => {
            let side = if o.action == Action::Buy { 1 } else { -1 };
            if cfg.pyramiding=="disabled" && d.account.positions.first().is_some_and(|p| p.side == side) {
                reason = "禁止同方向加碼".into();
            } else {
                if d.account.positions.first().is_some_and(|p|p.side != side) {
                    let mut reversal = pending.clone();
                    reversal.order.quantity = None;
                    d.account.mark(price, tv);
                    close_position(d, &reversal, price, bar, b, phase, cfg, tv)?;
                }
                let quantity = o.quantity.unwrap_or(cfg.contracts);
                let fee = money(cfg.commission_per_side)? * quantity as i128;
                let margin = cfg.margin_per_contract.map(money).transpose()?.unwrap_or(0)
                    * (d.account.quantity()+quantity) as i128;
                if cfg.max_position_contracts.is_some_and(|limit| d.account.quantity()+quantity > limit)
                    || cfg.max_entry_batches.is_some_and(|limit| d.account.positions.len()+1 > limit) {
                    reason = "超過總持倉口數或同方向進場批次上限".into();
                } else if (cfg.margin_per_contract.is_some() && d.account.cash - fee < margin)
                    || (cfg.enforce_cash_balance && d.account.cash - fee <= 0)
                {
                    reason = "資金或設定保證金不足".into();
                } else {
                    d.account.cash -= fee;
                    d.account.paid_fees += fee;
                    d.account.positions.push(Position {
                        order_id: pending.id,
                        side,
                        quantity,
                        price,
                        bar,
                        label: o.label.clone(),
                        datetime: b.datetime.clone(),
                        entry_fee: fee,
                        mfe: 0,
                        mae: 0,
                    });
                    d.fills.push(Fill {
                        is_entry: true,
                        order_id: pending.id,
                        bar,
                        datetime: b.datetime.clone(),
                        phase: phase.into(),
                        action: o.action.name().into(),
                        label: o.label.clone(),
                        side,
                        quantity,
                        price: price as f64 * cfg.tick_size,
                        fee: amount(fee),
                    });
                }
            }
        }
    }
    d.account.mark(base, tv);
    d.events.push(Event {
        raw_order_price: pending.order.price,
        order_id: pending.id,
        bar,
        label: o.label.clone(),
        status: if reason.is_empty() {
            "filled"
        } else {
            "ignored_or_rejected"
        }
        .into(),
        reason,
    });
    Ok(())
}
fn simulate(
    account: &Account,
    orders: &[Pending],
    bar: usize,
    b: &Candle,
    cfg: &Config,
    high_first: bool,
    tv: i128,
) -> Result<Delta, String> {
    let mut d = Delta {
        account: account.clone(),
        fills: vec![],
        trades: vec![],
        events: vec![],
    };
    let mut used = vec![false; orders.len()];
    let o = ticks(b.open, cfg.tick_size, true)?;
    let h = ticks(b.high, cfg.tick_size, true)?;
    let l = ticks(b.low, cfg.tick_size, true)?;
    let c = ticks(b.close, cfg.tick_size, true)?;
    let points = if high_first {
        [o, h, l, c]
    } else {
        [o, l, h, c]
    };
    let mut current = o;
    d.account.mark(o, tv);
    for (step, end) in points.into_iter().enumerate() {
        loop {
            let mut candidate: Option<(usize, i64, i64, u8)> = None;
            let mut mc_primary: Option<(usize, i64, i64, u8)> = None;
            let mut mc_fallback: Option<(usize, i64, i64, u8)> = None;
            let market_position = d.account.positions.first().map_or(0, |p| p.side);
            for (i, pending) in orders.iter().enumerate() {
                if used[i] {
                    continue;
                }
                let order = &pending.order;
                // An order cannot execute until its corresponding position is active.
                // Keep it pending: another order may change that position in this bar.
                if !active(order, &d.account, cfg) {
                    continue;
                }
                if order.kind == Kind::Market {
                    if step == 0 {
                        candidate = Some((i, current, 0, 0));
                        break;
                    }
                    continue;
                }
                if order.kind == Kind::Close {
                    continue;
                }
                let next = if trigger(order, current, cfg)? {
                    Some(current)
                } else if trigger(order, end, cfg)? {
                    Some(order_ticks(order, cfg)?)
                } else {
                    None
                };
                if let Some(px) = next {
                    let distance = (px - current).abs();
                    if let Some(mode) = cfg.mc_order_priority_mode {
                        // MC 的 flat-position 分支按委託列順序找第一張進場單。
                        if market_position == 0 {
                            if candidate.is_none() {
                                candidate = Some((i, px, distance, 0));
                            }
                            continue;
                        }

                        // 主要選擇器先從跨越目前路徑區間的委託中，取表值最高者；
                        // 表值為 0 時交由後備選擇器按最近觸發價處理。
                        let priority = mc_order_priority(order.action, market_position, mode)
                            .unwrap_or(0);
                        if priority > 0
                            && mc_primary.is_none_or(|(_, _, _, best)| priority > best)
                        {
                            mc_primary = Some((i, px, distance, priority));
                        }
                        let fallback_better = mc_fallback.is_none_or(
                            |(_, _, best_distance, best_priority)| {
                                distance < best_distance
                                    || (distance == best_distance && priority > best_priority)
                            },
                        );
                        if fallback_better {
                            mc_fallback = Some((i, px, distance, priority));
                        }
                    } else if candidate.is_none_or(|(_, _, dist, _)| distance < dist) {
                        candidate = Some((i, px, distance, 0));
                    }
                }
            }
            let candidate = if cfg.mc_order_priority_mode.is_some() {
                candidate.or(mc_primary).or(mc_fallback)
            } else {
                candidate
            };
            let Some((i, px, _, _)) = candidate else {
                break;
            };
            current = px;
            d.account.mark(px, tv);
            used[i] = true;
            execute(
                &mut d,
                &orders[i],
                px,
                bar,
                b,
                if step == 0 {
                    "open"
                } else {
                    "intrabar_assumed"
                },
                cfg,
                tv,
            )?;
        }
        current = end;
        d.account.mark(end, tv);
    }
    for (i, p) in orders.iter().enumerate() {
        if !used[i] {
            d.events.push(Event {
                raw_order_price: p.order.price,
                order_id: p.id,
                bar,
                label: p.order.label.clone(),
                status: "expired".into(),
                reason: "次棒未成交；有效期僅一根 K 棒".into(),
            });
        }
    }
    Ok(d)
}

fn simulate_ticks(account: &Account, orders: &[Pending], bar: usize, b: &Candle,
    cfg: &Config, sequence: &[crate::tick_execution::ExecutionTick], tv: i128) -> Result<Delta,String> {
    if sequence.is_empty() {return Err("逐筆撮合不得使用空Tick序列".into());}
    let mut d=Delta {account:account.clone(),fills:vec![],trades:vec![],events:vec![]};
    let mut used=vec![false;orders.len()];
    for (index,tick) in sequence.iter().enumerate() {
        let price=ticks(tick.price,cfg.tick_size,true)?;
        d.account.mark(price,tv);
        // Equal timestamp records retain source order. Within one print, source order
        // breaks order ties; re-check activity after every fill, without reusing orders.
        loop {
            let mut candidate=None;
            for (i,p) in orders.iter().enumerate() {
                if used[i] || !active(&p.order,&d.account,cfg) {continue;}
                if p.order.kind==Kind::Market && index!=0 {continue;}
                if trigger(&p.order,price,cfg)? {candidate=Some(i);break;}
            }
            let Some(i)=candidate else {break};
            used[i]=true;
            let base=if orders[i].order.kind==Kind::Limit {order_ticks(&orders[i].order,cfg)?} else {price};
            let mut at_tick=b.clone();at_tick.datetime=tick.datetime();
            execute(&mut d,&orders[i],base,bar,&at_tick,"tick",cfg,tv)?;
            d.account.mark(price,tv);
        }
    }
    for (i,p) in orders.iter().enumerate() {
        if !used[i] { d.events.push(Event {raw_order_price:p.order.price,order_id:p.id,bar,
            label:p.order.label.clone(),status:"expired".into(),reason:"次棒Tick未成交；有效期僅一根K棒".into()}); }
    }
    Ok(d)
}
#[derive(Debug, Serialize)]
pub struct Summary {
    pub trades: usize,
    pub winning: usize,
    pub losing: usize,
    pub breakeven: usize,
    pub net_closed: f64,
    pub gross_profit: f64,
    pub gross_loss: f64,
    pub profit_factor: Option<f64>,
    pub win_rate: Option<f64>,
    pub average_trade: Option<f64>,
    pub commissions: f64,
    pub open_pnl: f64,
    pub net_total: f64,
    pub final_equity: f64,
    pub max_drawdown: f64,
    pub max_drawdown_pct: f64,
}
#[derive(Debug, Serialize)]
pub struct OpenTrade {
    pub id: usize,
    pub entry_order_id: usize,
    pub side: i64,
    pub quantity: i64,
    pub entry_bar: usize,
    pub entry_datetime: String,
    pub entry_label: String,
    pub entry_price: f64,
    pub exit_bar: usize,
    pub exit_datetime: String,
    pub exit_price: f64,
    pub net: f64,
    pub cumulative: f64,
    pub mfe: f64,
    pub mae: f64,
}
#[derive(Debug, Serialize)]
pub struct VariableObservation {
    pub evaluated_bars: usize,
    pub nonzero_bars: usize,
    pub missing_bars: usize,
    pub minimum: Option<f64>,
    pub maximum: Option<f64>,
    pub last: Option<f64>,
}
#[derive(Debug, Serialize)]
pub struct ResultReport {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tick_execution: Option<crate::tick_execution::TickExecutionProof>,
    pub variable_observations: BTreeMap<String, VariableObservation>,
    pub schema_version: u32,
    pub engine_version: String,
    pub engine_sha256: String,
    pub configuration_sha256: String,
    pub strategy_sha256: String,
    pub config: Config,
    pub provenance: Provenance,
    pub inputs: BTreeMap<String, f64>,
    pub warnings: Vec<String>,
    pub first_bar: String,
    pub last_bar: String,
    pub bar_count: usize,
    pub ambiguous_bars: usize,
    pub summary: Summary,
    pub trades: Vec<Trade>,
    pub open_trade: Option<OpenTrade>,
    pub fills: Vec<Fill>,
    pub events: Vec<Event>,
    pub equity: Vec<Equity>,
    pub final_position: i64,
    pub pending_at_end: usize,
}
pub fn run(
    bars: &[Candle],
    program: &Program,
    cfg: &Config,
    provenance: Provenance,
    source: &str,
) -> Result<ResultReport, String> {
    if cfg.tick_dataset_id.is_some() {return Err("Tick模式必須呼叫run_with_database_ticks，禁止退回OHLC".into());}
    run_inner(bars,program,cfg,provenance,source,None)
}

pub fn run_with_database_ticks(database: &Path, bars: &[Candle], program: &Program,
    cfg: &Config, provenance: Provenance, source: &str) -> Result<ResultReport,String> {
    let mut feed=crate::tick_execution::DatabaseTicks::open(database,cfg,&provenance.symbol,&provenance.timezone)?;
    run_inner(bars,program,cfg,provenance,source,Some(&mut feed))
}

fn run_inner(bars: &[Candle], program: &Program, cfg: &Config, provenance: Provenance,
    source: &str, mut tick_feed: Option<&mut crate::tick_execution::DatabaseTicks>) -> Result<ResultReport,String> {
    cfg.validate()?;
    let mut configured_program = program.clone();
    configured_program.set_simple_variables(&cfg.simple_variables)?;
    configured_program.set_function_storage_types(&cfg.function_storage_types)?;
    let program = &configured_program;
    if bars.len() <= cfg.warmup_bars || bars.len() > 1_000_000 {
        return Err("K 棒數不足或超過第一版支援上限 1000000".into());
    }
    let tv = money(cfg.tick_size * cfg.point_value)?;
    let initial = money(cfg.initial_capital)?;
    let mut account = Account {
        cash: initial,
        positions: vec![],
        peak: initial,
        max_drawdown: 0,
        max_drawdown_pct: 0.0,
        trade_count: 0,
        closed_net: 0,
        bar_equity_low: initial,
        bar_equity_high: initial,
        bar_equity_low_after_high: initial,
        bar_drawdown: 0,
        paid_fees: 0,
        bar_gross_equity_low: initial,
        bar_position_equity_low: i128::MAX,
    };
    let mut rt = Runtime::new(program, bars);
    rt.bar_offset = cfg.warmup_bars;
    let mut pending: Vec<Pending> = vec![];
    let mut trades = vec![];
    let mut fills = vec![];
    let mut events = vec![];
    let mut equity = vec![];
    let mut next_id = 1;
    let mut ambiguous = 0;
    let mut recent_entry_dates: Vec<i32> = vec![];
    let mut variable_observations = BTreeMap::new();
    let register_entries = |dates: &mut Vec<i32>, fills: &[Fill], date: i32| {
        for _ in fills.iter().filter(|f| f.is_entry) {
            dates.push(date);
            if dates.len() > 10 {
                dates.remove(0);
            }
        }
    };
    for (i, b) in bars.iter().enumerate() {
        // Observe only this bar's selected execution path; do not include the
        // preceding close in a gap bar's high/low range.
        account.bar_equity_low = i128::MAX;
        account.bar_equity_high = i128::MIN;
        account.bar_equity_low_after_high = i128::MAX;
        account.bar_drawdown = 0;
        account.bar_gross_equity_low = i128::MAX;
        account.bar_position_equity_low = i128::MAX;
        let selected = if let Some(feed)=tick_feed.as_deref_mut() {
            let sequence=feed.bar(b)?;
            if i%10000==0 {eprintln!("Tick逐筆撮合：{}/{}根K棒",i,bars.len());}
            simulate_ticks(&account,&pending,i,b,cfg,&sequence,tv)?
        } else {
        let a = simulate(&account, &pending, i, b, cfg, true, tv)?;
        let z = simulate(&account, &pending, i, b, cfg, false, tv)?;
        let cp = ticks(b.close, cfg.tick_size, true)?;
        let differs = a
            .fills
            .iter()
            .map(|x| (&x.label, x.price, x.side))
            .collect::<Vec<_>>()
            != z.fills
                .iter()
                .map(|x| (&x.label, x.price, x.side))
                .collect::<Vec<_>>();
        if differs {
            ambiguous += 1;
        }
        let selected = if cfg.intrabar_policy == IntrabarPolicy::NearestExtreme {
            // Equal distances explicitly use the low-first path.
            if b.high - b.open < b.open - b.low {
                a
            } else {
                z
            }
        } else if differs {
            let av = a.account.equity(cp, tv);
            let zv = z.account.equity(cp, tv);
            if zv < av || (zv == av && z.account.max_drawdown < a.account.max_drawdown) {
                z
            } else {
                a
            }
        } else {
            a
        };
        selected
        };
        let cp = ticks(b.close, cfg.tick_size, true)?;
        register_entries(&mut recent_entry_dates, &selected.fills, b.pl_date);
        account = selected.account;
        trades.extend(selected.trades);
        fills.extend(selected.fills);
        events.extend(selected.events);
        pending.clear();
        if i >= cfg.warmup_bars {
            let (side, price, qty) = account.positions.first().map_or((0, 0.0, 0), |p| {
                (p.side, p.price as f64 * cfg.tick_size, account.quantity())
            });
            let minute = b.pl_time / 100 * 60 + b.pl_time % 100;
            rt.execution = ExecutionContext {
                total_trades: Some(account.trade_count as f64),
                ticks: cfg.build_volume_on.as_ref().map(|_| b.volume),
                big_point_value: Some(cfg.point_value),
                price_scale: cfg.price_scale,
                min_move: cfg.minimum_move.map(|v|v as f64),
                session_last_bar: if cfg.timestamp_role == "end" && !cfg.sessions.is_empty() {
                    Some(cfg.sessions.iter().any(|s| s.end_minute == minute))
                } else if cfg.interval_minutes == 0 && cfg.interval_seconds.is_none() {
                    Some(true)
                } else {
                    None
                },
                recent_entry_dates: recent_entry_dates.clone(),
                net_profit: Some(amount(account.closed_net)),
                recent_exits: trades.iter().rev().take(10).rev().map(|t| (t.exit_bar, t.exit_label.clone())).collect(),
                recent_exit_prices: trades.iter().rev().take(10).rev().map(|t| t.exit_price).collect(),
                entry_bar: account.positions.first().map(|p| p.bar),
                entry_name: account.positions.first().map(|p|p.label.clone()),
                recent_entry_names: trades.iter().rev().take(10).rev().map(|t|t.entry_label.clone()).collect(),
                open_entry_max_profit: Some(
                    account.positions.first().map_or(0.0, |p| {
                        amount(if cfg.mc_open_entry_max_profit_net.unwrap_or(true) {
                            (p.mfe - p.entry_fee).max(0)
                        } else {
                            p.mfe
                        })
                    }),
                ),
                open_entry_commission: Some(
                    amount(account.positions.iter().map(|p|p.entry_fee).sum()),
                ),
            };
            let generated = rt
                .step(i, side, price, qty)
                .map_err(|e| format!("{}：{e}", b.datetime))?;
            for name in program.variables.keys() {
                let v = rt.current_variable(name);
                let observation =
                    variable_observations
                        .entry(name.clone())
                        .or_insert(VariableObservation {
                            evaluated_bars: 0,
                            nonzero_bars: 0,
                            missing_bars: 0,
                            minimum: None,
                            maximum: None,
                            last: None,
                        });
                observation.evaluated_bars += 1;
                observation.last = v;
                if let Some(v) = v {
                    observation.nonzero_bars += (v != 0.0) as usize;
                    observation.minimum = Some(observation.minimum.map_or(v, |old| old.min(v)));
                    observation.maximum = Some(observation.maximum.map_or(v, |old| old.max(v)));
                } else {
                    observation.missing_bars += 1;
                }
            }
            let mut close = Delta {
                account: account.clone(),
                trades: vec![],
                fills: vec![],
                events: vec![],
            };
            for order in generated {
                let p = Pending { id: next_id, order };
                next_id += 1;
                if p.order.kind == Kind::Close {
                    execute(&mut close, &p, cp, i, b, "this_bar_close", cfg, tv)?;
                } else {
                    events.push(Event {
                        raw_order_price: p.order.price,
                        order_id: p.id,
                        bar: i,
                        label: p.order.label.clone(),
                        status: "generated".into(),
                        reason: "收盤判斷，次棒才可成交".into(),
                    });
                    pending.push(p);
                }
            }
            register_entries(&mut recent_entry_dates, &close.fills, b.pl_date);
            account = close.account;
            trades.extend(close.trades);
            fills.extend(close.fills);
            events.extend(close.events);
        }
        account.mark(cp, tv);
        equity.push(Equity {
            datetime: b.datetime.clone(),
            equity: amount(account.equity(cp, tv)),
            cash: amount(account.cash),
            position: account.positions.first().map_or(0, |p| p.side * account.quantity()),
            drawdown: amount(account.equity(cp, tv) - account.peak),
            equity_low: amount(account.bar_equity_low),
            equity_high: amount(account.bar_equity_high),
            equity_low_after_high: amount(account.bar_equity_low_after_high),
            bar_drawdown: amount(account.bar_drawdown),
            gross_equity_low: amount(account.bar_gross_equity_low),
            gross_equity_close: amount(account.equity(cp, tv) + account.paid_fees),
            position_equity_low: (account.bar_position_equity_low != i128::MAX)
                .then(|| amount(account.bar_position_equity_low)),
        });
    }
    let gross_profit: f64 = trades
        .iter()
        .filter(|t| t.net > 0.0)
        .map(|t| t.net)
        .sum::<f64>()
        + 0.0;
    let gross_loss: f64 = trades
        .iter()
        .filter(|t| t.net < 0.0)
        .map(|t| t.net)
        .sum::<f64>()
        + 0.0;
    let winning = trades.iter().filter(|t| t.net > 0.0).count();
    let losing = trades.iter().filter(|t| t.net < 0.0).count();
    let final_equity = equity.last().unwrap().equity;
    let commissions = fills.iter().map(|f| f.fee).sum::<f64>() + 0.0;
    let open_pnl = account.positions.first().map_or(0.0, |p| {
        amount(
            (ticks(bars.last().unwrap().close, cfg.tick_size, true).unwrap() - p.price) as i128
                * p.side as i128
                * p.quantity as i128
                * tv
                - p.entry_fee,
        )
    });
    let summary = Summary {
        trades: trades.len(),
        winning,
        losing,
        breakeven: trades.len() - winning - losing,
        net_closed: amount(account.closed_net),
        gross_profit,
        gross_loss,
        profit_factor: if gross_loss < 0.0 {
            Some(gross_profit / -gross_loss)
        } else {
            None
        },
        win_rate: if trades.is_empty() {
            None
        } else {
            Some(winning as f64 / trades.len() as f64 * 100.0)
        },
        average_trade: if trades.is_empty() {
            None
        } else {
            Some(amount(account.closed_net) / trades.len() as f64)
        },
        commissions,
        open_pnl,
        net_total: final_equity - cfg.initial_capital,
        final_equity,
        max_drawdown: amount(account.max_drawdown),
        max_drawdown_pct: account.max_drawdown_pct,
    };
    let mut warnings = vec![
        "研究引擎 v0.6，預設依本機MC原生價格取整與近端極值路徑；完整相容範圍以各項驗證紀錄為準".into(),
        "限價採觸價完全成交，沒有委託簿排隊、部分成交、漲跌幅或稅費分拆模型".into(),
        "當棒收盤單是回測假設，不能保證真實收盤可成交".into(),
        "未實作假日曆、特殊收盤、轉倉與日夜盤交易日歸屬；指定交易時段不等於已校對 MC 時段".into(),
        "MFE 扣進場佣金並以零為下限；MAE 扣進場佣金，另保留毛變動".into(),
    ];
    if !matches!(cfg.price_rounding,PriceRounding::McNative|PriceRounding::McNativeOnePoint) {
        warnings.push(format!("已明示使用歷史／診斷取整模式 {:?}，不代表共用MC原生取整預設",cfg.price_rounding));
    }
    if tick_feed.is_some() {
        warnings.push("訊號僅在K棒收盤計算；次棒依canonical Tick原始順序撮合，停損穿價採觸發成交價；限價採委託價觸價全成，未模擬排隊".into());
    } else { warnings.push(
        match cfg.intrabar_policy {
            IntrabarPolicy::Adverse => {
                "OHLC 歧義棒選當棒收盤權益較差路徑；非逐筆還原，不保證全期間最差結果"
            }
            IntrabarPolicy::NearestExtreme => {
                "OHLC 先走離開盤較近的極值，等距先低；此為明示模擬假設，非逐筆還原"
            }
        }
        .into(),
    ); }
    if cfg.margin_per_contract.is_none() {
        warnings.push("未設定保證金模型，資金可行性不能據此判定".into());
    }
    if !cfg.enforce_cash_balance {
        warnings.push("研究設定不限制負權益後的新倉；此模式不代表真實可交易資金".into());
    }
    if provenance.source_timestamp_role == "unspecified" {
        warnings.push("來源時間起訖未確認，使用任務明示的時間標記假設".into());
    }
    if provenance.timezone == "unspecified" {
        warnings.push("來源時區未知，保留原始時鐘，不推定為交易所時間".into());
    }
    if provenance.partial_bars > 0 {
        warnings.push(format!(
            "{} 根彙整 K 棒的來源筆數不足完整分鐘數；保留並標示，未填補",
            provenance.partial_bars
        ));
    }
    if account.positions.len()>1 { return Err("多批未平倉明細匯出尚未支援；請先平倉，不會遺漏批次後交付報告".into()); }
    let final_position = account.positions.first().map_or(0, |p| p.side * account.quantity());
    let open_trade = account.positions.first().map(|p| {
        let last = bars.last().unwrap();
        let close = ticks(last.close, cfg.tick_size, true).unwrap();
        let gross = (close - p.price) as i128 * p.side as i128 * p.quantity as i128 * tv;
        let net = gross - p.entry_fee;
        OpenTrade {
            id: trades.len() + 1,
            entry_order_id: p.order_id,
            side: p.side,
            quantity: p.quantity,
            entry_bar: p.bar,
            entry_datetime: p.datetime.clone(),
            entry_label: p.label.clone(),
            entry_price: p.price as f64 * cfg.tick_size,
            exit_bar: bars.len() - 1,
            exit_datetime: last.datetime.clone(),
            exit_price: close as f64 * cfg.tick_size,
            net: amount(net),
            cumulative: amount(account.closed_net + net),
            mfe: amount((p.mfe - p.entry_fee).max(0)),
            mae: amount(p.mae - p.entry_fee),
        }
    });
    if source.to_ascii_lowercase().contains("rsi") || cfg.functions.values().any(|s|s.to_ascii_lowercase().contains("rsi")) {
        warnings.push("RSI 依官方 EasyLanguage 的 Wilder 平滑公式新增支援；固定整數長度，初始平價區間拒絕代填。尚未取得原生 MC RSI 逐棒對照，不宣稱完全等價".into());
    }
    if source.to_ascii_lowercase().contains("averagefc") {
        warnings.push("AverageFC 使用逐棒累加更新；HighestFC／LowestFC 使用完整窗格高低值。尚未驗證與MC各版本浮點計算逐位元一致".into());
    }
    if source.to_ascii_lowercase().contains("openentrymaxprofit") {
        if cfg.mc_open_entry_max_profit_net.unwrap_or(true) {
            warnings.push("OpenEntryMaxProfit(0) 預設以最大浮動獲利扣進場佣金；此口徑已使KAIv7標準案例逐筆及摘要與MC一致，其他策略／MC版本仍須比對".into());
        } else {
            warnings.push("OpenEntryMaxProfit(0) 已明示切為未扣進場佣金的 gross 診斷語意，不代表 MC 相容預設".into());
        }
    }
    if source.to_ascii_lowercase().contains("entriestoday") {
        warnings.push(
            "EntriesToday 按成交棒日曆日期計算最近10次進場；非交易所跨夜交易日，未支援加碼進場"
                .into(),
        );
    }
    if source.to_ascii_lowercase().contains("ticks") {
        warnings.push("Ticks 依明示 Build Volume On=trade_volume 回傳彙整成交量；依先前圖表成交股／口數設定，不等於成交筆數模式".into());
    }
    if program.functions.values().any(|f| !f.parameters.is_empty()) {
        warnings.push("帶參數函式依呼叫位置各自維護 NumericSeries 歷史；未宣稱與 MC 編譯器的 StorageType 生命週期等價。Fix1 的原生儲存型態修正仍須由 MC 回測驗證".into());
        warnings.push("研究引擎 AvgTrueRange 採 TrueRange 算術平均，Round 採中點離零，IFF 僅取選定分支，XAverage 以同一呼叫位置的前值及當棒長度遞迴；尚未逐棒與 YAO1 原始 MC 內建函式對照".into());
    }
    let mut fingerprint = Sha256::new();
    for bytes in [
        include_bytes!("backtest.rs").as_slice(),
        include_bytes!("tick_execution.rs").as_slice(),
        include_bytes!("tick_bars.rs").as_slice(),
        include_bytes!("powerlanguage.rs").as_slice(),
        include_bytes!("database.rs").as_slice(),
        include_bytes!("../Cargo.lock").as_slice(),
    ] {
        fingerprint.update(bytes);
    }
    Ok(ResultReport {
        tick_execution: tick_feed.as_deref().map(|f|f.proof()),
        variable_observations,
        schema_version: 2,
        engine_version: "recharts-backtest-0.6".into(),
        engine_sha256: format!("{:x}", fingerprint.finalize()),
        configuration_sha256: format!(
            "{:x}",
            Sha256::digest(serde_json::to_vec(cfg).map_err(|e| e.to_string())?)
        ),
        strategy_sha256: format!("{:x}", Sha256::digest(source.as_bytes())),
        config: cfg.clone(),
        provenance,
        inputs: program.inputs.clone(),
        warnings,
        first_bar: bars.first().unwrap().datetime.clone(),
        last_bar: bars.last().unwrap().datetime.clone(),
        bar_count: bars.len(),
        ambiguous_bars: ambiguous,
        summary,
        trades,
        open_trade,
        fills,
        events,
        equity,
        final_position,
        pending_at_end: pending.len(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn scale_fill(d:&mut Delta,c:&Config,action:Action,label:&str,q:i64,total:bool,price:i64) {
        let p=Pending{id:d.fills.len()+1,order:Order{total,action,label:label.into(),quantity:Some(q),price:None,kind:Kind::Market}};
        execute(d,&p,price,0,&bars()[0],"open",c,money(c.tick_size*c.point_value).unwrap()).unwrap();
    }
    fn scale_config()->Config {
        let mut c=cfg(); c.pyramiding="any_signal".into(); c.max_position_contracts=Some(5); c.max_entry_batches=Some(5); c
    }
    #[test]
    fn same_signal_scaling_obeys_total_contract_and_batch_caps() {
        let mut c=scale_config();
        let mut d=Delta{account:tick_account(),fills:vec![],trades:vec![],events:vec![]};
        scale_fill(&mut d,&c,Action::Buy,"same",3,false,100);
        scale_fill(&mut d,&c,Action::Buy,"same",3,false,101);
        assert_eq!(d.account.quantity(),3); assert_eq!(d.events.last().unwrap().status,"ignored_or_rejected");
        scale_fill(&mut d,&c,Action::Buy,"same",2,false,101);
        assert_eq!(d.account.quantity(),5);assert_eq!(d.account.positions.len(),2);
        c.max_entry_batches=Some(1);
        scale_fill(&mut d,&c,Action::Sell, "trim",1,true,102);
        scale_fill(&mut d,&c,Action::Buy,"other",1,false,102);
        assert_eq!(d.account.quantity(),4); assert_eq!(d.account.positions.len(),2);
        c.pyramiding="different_signal".into();assert!(c.validate().is_err());
    }
    #[test]
    fn fifo_partial_exits_split_lots_costs_and_excursions_for_both_sides() {
        for side in [1,-1] {
            let c=scale_config();
            let mut d=Delta{account:tick_account(),fills:vec![],trades:vec![],events:vec![]};
            let entry=if side==1 {Action::Buy}else{Action::SellShort};
            let exit=if side==1 {Action::Sell}else{Action::BuyToCover};
            scale_fill(&mut d,&c,entry,"first",1,false,100);
            scale_fill(&mut d,&c,entry,"second",2,false,100+side*10);
            scale_fill(&mut d,&c,entry,"third",2,false,100+side*20);
            scale_fill(&mut d,&c,exit,"partial",2,true,100+side*30);
            assert_eq!(d.account.quantity(),3); assert_eq!(d.trades.len(),2);
            assert_eq!(d.trades[0].entry_label,"first");assert_eq!(d.trades[1].entry_label,"second");
            assert_eq!(d.trades[0].net,4400.0);assert_eq!(d.trades[1].net,2400.0);
            assert_eq!(d.trades[1].mfe,3200.0); assert_eq!(d.account.positions[0].entry_fee,money(800.0).unwrap());
            scale_fill(&mut d,&c,exit,"rest",3,true,100+side*30);
            assert_eq!(d.account.quantity(),0);assert_eq!(d.trades.len(),4);
            assert_eq!(d.trades.iter().map(|t|t.net).sum::<f64>(),10000.0);
            assert_eq!(d.fills.iter().map(|f|f.fee).sum::<f64>(),8000.0);
        }
    }
    #[test]
    fn explicit_quantity_without_total_applies_to_each_entry_not_whole_position() {
        for total in [false,true] {
            let c=scale_config();let mut d=Delta{account:tick_account(),fills:vec![],trades:vec![],events:vec![]};
            scale_fill(&mut d,&c,Action::Buy,"a",2,false,100);
            scale_fill(&mut d,&c,Action::Buy,"b",3,false,100);
            scale_fill(&mut d,&c,Action::Sell,"exit",1,total,100);
            assert_eq!(d.account.quantity(),if total {4}else{3});
            assert_eq!(d.trades.len(),if total {1}else{2});
        }
    }
    fn tick_sequence(prices: &[f64]) -> Vec<crate::tick_execution::ExecutionTick> {
        prices.iter().enumerate().map(|(i,p)|crate::tick_execution::ExecutionTick {
            ns:1_600_000_000_000_000_000+i as i64,price:*p,volume:1_000_000 }).collect()
    }
    fn tick_account() -> Account {
        Account {cash:100_000_000_000,positions:vec![],peak:100_000_000_000,max_drawdown:0,
            max_drawdown_pct:0.0,trade_count:0,closed_net:0,
            bar_equity_low:100_000_000_000,bar_equity_high:100_000_000_000,bar_equity_low_after_high:100_000_000_000,bar_drawdown:0,
            paid_fees:0,bar_gross_equity_low:100_000_000_000,bar_position_equity_low:i128::MAX}
    }
    #[test]
    fn post_peak_low_restarts_at_the_last_equal_bar_high() {
        let mut a = tick_account();
        a.bar_equity_high = i128::MIN;
        a.bar_equity_low_after_high = i128::MAX;
        for cash in [100, 120, 90, 120, 110] {
            a.cash = cash;
            a.mark(0, 1);
        }
        assert_eq!(a.bar_equity_low_after_high, 110);
        assert_eq!(a.bar_equity_high, 120);
        assert_eq!(a.bar_equity_low, 90);
    }
    fn tick_orders() -> Vec<Pending> {
        let p=Program::compile("Buy next bar at market; Sell (\"SL\") next bar at 95 stop; Sell (\"TP\") next bar at 105 limit;",&BTreeMap::new()).unwrap();
        let b=bars();let mut rt=Runtime::new(&p,&b);
        rt.step(0,0,0.0,0).unwrap().into_iter().enumerate().map(|(i,order)|Pending{id:i+1,order}).collect()
    }
    #[test]
    fn actual_tick_order_resolves_identical_ohlc_and_keeps_real_timestamp() {
        let mut c=cfg();c.commission_per_side=0.0;c.point_value=1.0;
        let b=bars();let orders=tick_orders();
        let high=simulate_ticks(&tick_account(),&orders,1,&b[1],&c,&tick_sequence(&[100.,110.,90.,100.]),money(1.).unwrap()).unwrap();
        let low=simulate_ticks(&tick_account(),&orders,1,&b[1],&c,&tick_sequence(&[100.,90.,110.,100.]),money(1.).unwrap()).unwrap();
        assert_eq!(high.trades[0].exit_label,"TP");assert_eq!(high.trades[0].exit_price,105.);
        assert_eq!(low.trades[0].exit_label,"SL");assert_eq!(low.trades[0].exit_price,90.);
        assert_ne!(low.trades[0].exit_datetime,b[1].datetime);
        assert_eq!(low.fills[0].phase,"tick");
        // The 110 print happened after the losing exit: no future MFE leakage.
        assert_eq!(low.trades[0].mfe_gross,0.);
    }
    #[test]
    fn tick_gap_does_not_fill_stop_at_untraded_level() {
        let mut c=cfg();c.commission_per_side=0.;c.point_value=1.;let b=bars();
        let d=simulate_ticks(&tick_account(),&tick_orders(),1,&b[1],&c,&tick_sequence(&[100.,90.]),money(1.).unwrap()).unwrap();
        assert_eq!(d.trades[0].exit_price,90.);assert_eq!(d.trades[0].net,-10.);
        assert_eq!(d.events.iter().filter(|e|e.status=="expired").count(),1);
        assert!(simulate_ticks(&tick_account(),&[],1,&b[1],&c,&[],money(1.).unwrap()).is_err());
    }
    #[test]
    fn same_timestamp_ticks_preserve_sequence_and_slippage_is_adverse() {
        let mut c=cfg();c.commission_per_side=0.;c.point_value=1.;c.slippage_ticks=1;
        let mut sequence=tick_sequence(&[100.,110.,90.]);
        for t in &mut sequence {t.ns=1_600_000_000_000_000_000;}
        let b=bars();let d=simulate_ticks(&tick_account(),&tick_orders(),1,&b[1],&c,&sequence,money(1.).unwrap()).unwrap();
        assert_eq!(d.trades[0].entry_price,101.);assert_eq!(d.trades[0].exit_label,"TP");
        assert_eq!(d.trades[0].exit_price,105.); // limit never worsens from slippage
    }
    #[test]
    fn tick_config_cannot_silently_use_ohlc() {
        let mut c=cfg();c.tick_dataset_id=Some(4);let b=bars();
        let p=Program::compile("Buy next bar at market;",&BTreeMap::new()).unwrap();
        let provenance=Provenance {outside_session_rows:0,dataset_id:1,symbol:"TXF1".into(),source:"test".into(),kind:"minute".into(),timezone:"Asia/Taipei".into(),source_timestamp_role:"end".into(),source_sha256:"".into(),bars_sha256:"".into(),source_rows:3,partial_bars:0};
        assert!(run(&b,&p,&c,provenance,"").unwrap_err().contains("禁止退回OHLC"));
    }
    fn cfg() -> Config {
        Config {
            pyramiding:"disabled".into(),max_position_contracts:None,max_entry_batches:None,
            interval_seconds: None,
            interval_days: None,
            simple_variables: vec![],
            function_storage_types: BTreeMap::new(),
            source_interval_minutes: None,
            tick_dataset_id: None,
            build_volume_on: None,
            strategy_name: "test".into(),
            price_rounding: PriceRounding::Directional,
            price_scale: Some(1.0),
            minimum_move: Some(1),
            mc_order_priority_mode: None,
            mc_open_entry_max_profit_net: None,
            intrabar_policy: IntrabarPolicy::Adverse,
            sessions: vec![],
            dataset_id: 1,
            from: "".into(),
            to: "".into(),
            interval_minutes: 0,
            bar_anchor_minutes: 0,
            timestamp_role: "end".into(),
            warmup_bars: 0,
            initial_capital: 100000.0,
            tick_size: 1.0,
            point_value: 200.0,
            commission_per_side: 800.0,
            slippage_ticks: 0,
            contracts: 1,
            margin_per_contract: None,
            enforce_cash_balance: true,
            inputs: BTreeMap::new(),
            functions: BTreeMap::new(),
            research_note: "test".into(),
        }
    }
    fn bars() -> Vec<Candle> {
        [
            (100.0, 105.0, 95.0, 102.0),
            (110.0, 120.0, 105.0, 115.0),
            (130.0, 135.0, 125.0, 132.0),
        ]
        .into_iter()
        .enumerate()
        .map(|(i, (open, high, low, close))| Candle {
            datetime: format!("2026-10-{:02}", i + 1),
            open,
            high,
            low,
            close,
            volume: 100.0,
            pl_date: 1261001 + i as i32,
            pl_time: 0,
            source_rows: 1,
        })
        .collect()
    }
    fn execute_source(src: &str, b: &[Candle], c: &Config) -> ResultReport {
        let p = Program::compile(src, &BTreeMap::new()).unwrap();
        run(
            b,
            &p,
            c,
            Provenance {
                outside_session_rows: 0,
                dataset_id: 1,
                symbol: "test".into(),
                source: "test".into(),
                kind: "daily".into(),
                timezone: "unspecified".into(),
                source_timestamp_role: "unspecified".into(),
                source_sha256: "test".into(),
                bars_sha256: "test".into(),
                source_rows: b.len(),
                partial_bars: 0,
            },
            src,
        )
        .unwrap()
    }
    #[test]
    fn next_bar_costs_and_terminal_pending() {
        let r=execute_source("if BarNumber = 1 then Buy next bar at market; if BarNumber = 2 then Sell next bar at market;",&bars(),&cfg());
        assert_eq!(r.trades.len(), 1);
        assert_eq!(r.trades[0].entry_price, 110.0);
        assert_eq!(r.trades[0].exit_price, 130.0);
        assert_eq!(r.summary.net_closed, 2400.0);
        assert_eq!(r.summary.commissions, 1600.0);
        assert_eq!(r.summary.final_equity, 102400.0);
    }
    #[test]
    fn completed_exit_updates_yao1_name_bars_and_net_profit_before_signal() {
        let source="Vars: recorded(0); if CurrentBar=1 then Buy next bar at market; if CurrentBar=2 then Sell (\"s%\") next bar at market; if ExitName(1)=\"s%\" and BarsSinceExit(1)=0 then recorded=NetProfit;";
        let r=execute_source(source,&bars(),&cfg());
        assert_eq!(r.trades.len(),1);assert_eq!(r.trades[0].exit_label,"s%");
        assert_eq!(r.variable_observations["recorded"].last,Some(2400.0));
    }
    #[test]
    fn open_entry_commission_comes_from_filled_position_and_scales_with_contracts() {
        let mut config = cfg();
        config.contracts = 2;
        let candles = bars();
        let result = execute_source("Vars: paid(0), afterflat(0); if BarNumber=1 then Buy next bar at market; if BarNumber=2 then begin paid=OpenEntryComission(0); Sell next bar at market; end; if BarNumber=3 then afterflat=OpenEntryComission(0);", &candles, &config);
        assert_eq!(result.variable_observations["paid"].last, Some(1600.0));
        assert_eq!(result.variable_observations["afterflat"].last, Some(0.0));
        assert_eq!(result.summary.commissions, 3200.0);
    }
    #[test]
    fn entries_today_counts_reversal_entry_once_and_tracks_open_peak() {
        let mut b = bars();
        for (i, bar) in b.iter_mut().enumerate() {
            bar.pl_date = 1261001;
            bar.datetime = format!("2026-10-01T09:{:02}:00", i);
        }
        let r=execute_source("Vars: n(0), peak(0); if BarNumber=1 then Buy next bar at market; if BarNumber=2 then begin peak=OpenEntryMaxProfit(0); SellShort next bar at market; end; n=EntriesToday(Date);",&b,&cfg());
        assert_eq!(r.fills.iter().filter(|f| f.is_entry).count(), 2);
        assert_eq!(r.variable_observations["n"].last, Some(2.0));
        assert_eq!(r.variable_observations["peak"].last, Some(1200.0));
    }
    #[test]
    fn supplied_ma5_original_stays_flat_despite_long_and_short_conditions() {
        let source = include_str!("../../strategies/KGI_08_MA5.user-provided.pl");
        let functions = BTreeMap::from([
            (
                "rea_fn_kgi_id".into(),
                include_str!("../../strategies/functions/REA_FN_KGI_ID.pl").into(),
            ),
            (
                "rea_fn_entryh_k".into(),
                include_str!("../../strategies/functions/REA_FN_EntryH_K.pl").into(),
            ),
            (
                "rea_fn_entryl_k".into(),
                include_str!("../../strategies/functions/REA_FN_EntryL_K.pl").into(),
            ),
            (
                "rea_fn_isld_k".into(),
                include_str!("../../strategies/functions/REA_FN_ISLD_K.pl").into(),
            ),
            (
                "rea_fnmp_k".into(),
                include_str!("../../strategies/functions/REA_FNMP_K.pl").into(),
            ),
        ]);
        let mut c = cfg();
        c.warmup_bars = 110;
        c.functions = functions;
        let start = NaiveDate::from_ymd_opt(2024, 7, 1).unwrap();
        let b: Vec<_> = (0..500)
            .map(|i| {
                let date = start + chrono::Duration::days(i);
                let close = 30000.0 + (i as f64 * 0.04).sin() * 2000.0;
                Candle {
                    datetime: date.to_string(),
                    open: close,
                    high: close.ceil() + 5.0,
                    low: close.floor() - 5.0,
                    close: close.round(),
                    volume: 10.0,
                    pl_date: (date.year() - 1900) * 10000
                        + date.month() as i32 * 100
                        + date.day() as i32,
                    pl_time: 1900,
                    source_rows: 1,
                }
            })
            .collect();
        let p = Program::with_functions(source, &c.inputs, &c.functions).unwrap();
        let mut provenance = Provenance {
            outside_session_rows: 0,
            dataset_id: 1,
            symbol: "test".into(),
            source: "test".into(),
            kind: "daily".into(),
            timezone: "unspecified".into(),
            source_timestamp_role: "end".into(),
            source_sha256: "test".into(),
            bars_sha256: "test".into(),
            source_rows: b.len(),
            partial_bars: 0,
        };
        // Use integer tick-aligned synthetic opens as well.
        let mut b = b;
        for bar in &mut b {
            bar.open = bar.close;
        }
        provenance.source_rows = b.len();
        let r = run(&b, &p, &c, provenance, source).unwrap();
        assert_eq!(r.summary.trades, 0);
        assert_eq!(r.fills.len(), 0);
        assert_eq!(r.variable_observations["continecost"].maximum, Some(0.0));
        assert!(r.variable_observations["entrylong"].nonzero_bars > 0);
        assert!(r.variable_observations["entryshort"].nonzero_bars > 0);
    }
    #[test]
    fn stop_gap_and_no_forced_final_close() {
        let r = execute_source(
            "if BarNumber = 1 then Buy next bar at 105 stop;",
            &bars(),
            &cfg(),
        );
        assert_eq!(r.fills[0].price, 110.0);
        assert_eq!(r.summary.trades, 0);
        assert_eq!(r.final_position, 1);
        assert_eq!(r.summary.open_pnl, 3600.0);
        assert_eq!(r.summary.net_total, 3600.0);
    }
    #[test]
    fn ambiguous_exit_uses_adverse_path() {
        let b = bars();
        let r=execute_source("if BarNumber = 1 then begin Buy next bar at market; Sell (\"SL\") next bar at 108 stop; Sell (\"TP\") next bar at 118 limit; end;",&b,&cfg());
        assert_eq!(r.trades[0].exit_label, "SL");
        assert_eq!(r.summary.net_closed, -2000.0);
        assert!(r.ambiguous_bars > 0);
    }
    #[test]
    fn nearest_extreme_and_excursion_costs() {
        let mut b = bars();
        b[1].open = 116.0;
        let mut c = cfg();
        c.intrabar_policy = IntrabarPolicy::NearestExtreme;
        let src = "if BarNumber = 1 then begin Buy next bar at market; Sell (\"SL\") next bar at 108 stop; Sell (\"TP\") next bar at 118 limit; end;";
        let r = execute_source(src, &b, &c);
        assert_eq!(r.trades[0].exit_label, "TP");
        assert_eq!(r.trades[0].mfe_gross, 400.0);
        assert_eq!(r.trades[0].mfe, 0.0);
        assert_eq!(r.trades[0].mae, -800.0);
        // Reporting telemetry must retain both fee-inclusive and gross path
        // extrema, including commissions charged after the intrabar exit.
        assert_eq!(r.equity[1].equity_low, 98800.0);
        assert_eq!(r.equity[1].equity_high, 100000.0);
        assert_eq!(r.equity[1].gross_equity_low, 100000.0);
        assert_eq!(r.equity[1].gross_equity_close, 100400.0);
        assert_eq!(r.equity[1].bar_drawdown, -1200.0);
        assert_eq!(r.equity[1].position_equity_low, Some(99200.0));
        assert_eq!(r.equity[2].position_equity_low, None);
        assert_eq!(r.equity[2].equity_high, 98800.0);
        b[1].open = 110.0;
        let r = execute_source(src, &b, &c);
        assert_eq!(r.trades[0].exit_label, "SL");
        // Equal distances choose the low first, an explicit deterministic tie rule.
        b[1].open = 112.5;
        c.tick_size = 0.5;
        let r = execute_source(src, &b, &c);
        assert_eq!(r.trades[0].exit_label, "SL");
    }
    #[test]
    fn entry_waits_for_exit_and_market_exit_waits_for_reversal() {
        let mut c = cfg();
        c.intrabar_policy = IntrabarPolicy::NearestExtreme;
        let r=execute_source("if BarNumber = 1 then Buy next bar at market; if BarNumber = 2 then begin Buy (\"reentry\") next bar at 130 stop; Sell (\"target\") next bar at 130 limit; end;",&bars(),&c);
        assert_eq!(r.trades.len(), 1);
        assert_eq!(r.final_position, 1);
        assert_eq!(r.fills.last().unwrap().label, "reentry");
        let r=execute_source("if BarNumber = 1 then SellShort next bar at market; if BarNumber = 2 then begin Sell (\"exit\") next bar at market; Buy (\"reverse\") next bar at market; end;",&bars(),&c);
        assert_eq!(r.trades.len(), 2);
        assert_eq!(r.trades[1].entry_bar, r.trades[1].exit_bar);
        assert_eq!(r.trades[1].net, -1600.0);
    }
    #[test]
    fn native_priority_tables_match_original_dll_selection_vectors() {
        #[derive(Deserialize)]
        struct Row {selector:i64,mode:u8,classes:Vec<usize>,allow_mask:u32,native_index:Option<usize>}
        #[derive(Deserialize)]
        struct Fixture {calls:usize,rows:Vec<Row>}
        let fixture:Fixture=serde_json::from_str(include_str!("../tests/fixtures/mc-order-priority-native.json")).unwrap();
        assert_eq!(fixture.calls,28_080);assert_eq!(fixture.rows.len(),fixture.calls);
        let actions=[Action::Buy,Action::BuyToCover,Action::SellShort,Action::Sell];
        for (index,row) in fixture.rows.iter().enumerate() {
            // Native selector0 returns the first entry without invoking callback;
            // the enclosing executor validates it separately before any fill.
            let selected=if row.selector==0 {row.classes.iter().position(|class|*class==0||*class==2)} else {
                let mut best=0;let mut selected=None;
                for (index,class) in row.classes.iter().enumerate() {
                    let priority=mc_order_priority(actions[*class],row.selector,row.mode).unwrap();
                    if priority>best && row.allow_mask&(1<<index)!=0 {best=priority;selected=Some(index);}
                }
                selected
            };
            assert_eq!(selected,row.native_index,"原DLL向量{index}: selector={} mode={} classes={:?} mask={}",row.selector,row.mode,row.classes,row.allow_mask);
        }
    }
    #[test]
    fn explicit_priority_modes_use_correct_table_for_reversal_and_exit_ties() {
        let mut c=cfg();c.mc_order_priority_mode=Some(0);
        let r=execute_source("if BarNumber=1 then SellShort next bar at market; if BarNumber=2 then begin Buy (\"reverse\") next bar at 200 limit; BuyToCover (\"cover\") next bar at 100 stop; end;",&bars(),&c);
        assert_eq!(r.trades[0].exit_label,"reverse");assert_eq!(r.final_position,1);
        c.mc_order_priority_mode=Some(1);
        let r=execute_source("if BarNumber=1 then Buy next bar at market; if BarNumber=2 then begin SellShort (\"reverse\") next bar at 1 limit; Sell (\"exit\") next bar at 200 stop; end;",&bars(),&c);
        assert_eq!(r.trades[0].exit_label,"reverse");assert_eq!(r.final_position,-1);
    }
    #[test]
    fn price_rounding_is_explicit() {
        let mut c = cfg();
        let order = Order {
            total:false,
            action: Action::BuyToCover,
            label: String::new(),
            quantity: None,
            kind: Kind::Stop,
            price: Some(44360.4),
        };
        assert_eq!(order_ticks(&order, &c).unwrap(), 44361);
        c.price_rounding = PriceRounding::Nearest;
        assert_eq!(order_ticks(&order, &c).unwrap(), 44360);
    }
    #[test]
    fn mc_native_one_point_snaps_only_near_integer_and_keeps_order_direction() {
        let mut c=cfg();c.tick_size=1.0;c.price_rounding=PriceRounding::McNativeOnePoint;
        for action in [Action::Buy,Action::BuyToCover,Action::Sell,Action::SellShort] {
            for kind in [Kind::Limit,Kind::Stop] {
                let up=matches!((kind,buy_side(action)),(Kind::Limit,false)|(Kind::Stop,true));
                for (raw,expected) in [(16962.005,16962),(47211.99296193164,47212),
                    (100.0,100),(100.009,100),(100.991,101),
                    (100.011,if up {101}else{100}),
                    (100.989,if up {101}else{100}),(100.5,if up {101}else{100}),
                    (0.01,0),(0.99,1)] {
                    let order=Order{total:false,action,label:String::new(),quantity:None,kind,price:Some(raw)};
                    assert_eq!(order_ticks(&order,&c).unwrap(),expected,"{action:?} {kind:?} {raw}");
                }
            }
        }
        let order=Order{total:false,action:Action::BuyToCover,label:String::new(),quantity:None,kind:Kind::Stop,price:Some(16962.005)};
        c.price_rounding=PriceRounding::Directional;
        assert_eq!(order_ticks(&order,&c).unwrap(),16963);
        c.price_rounding=PriceRounding::McNativeOnePoint;c.tick_size=0.5;
        assert!(c.validate().is_err());assert!(order_ticks(&order,&c).is_err());
        c.tick_size=1.0;let negative=Order{total:false,price:Some(-1.0),..order};
        assert!(order_ticks(&negative,&c).is_err());
    }
    #[test]
    fn mc_one_point_rounding_matches_recorded_original_dll_calls() {
        // 預期值由獨立程序實際呼叫原廠函式取得，並非由 Rust 實作產生。
        let fixture:serde_json::Value=serde_json::from_str(include_str!("../tests/fixtures/mc-one-point-price-native.json")).unwrap();
        let mut c=cfg();c.tick_size=1.0;c.price_rounding=PriceRounding::McNativeOnePoint;
        for row in fixture["rows"].as_array().unwrap() {
            let bits=row["price_bits"].as_str().unwrap().parse::<u64>().unwrap();
            let order=Order{total:false,action:if row["direction"].as_i64().unwrap()>0 {Action::BuyToCover}else{Action::Sell},
                label:String::new(),quantity:None,kind:Kind::Stop,price:Some(f64::from_bits(bits))};
            assert_eq!(order_ticks(&order,&c).unwrap(),row["native"].as_i64().unwrap(),"{row}");
        }
    }
    #[test]
    fn mc_native_common_helper_matches_all_original_dll_general_vectors() {
        let fixture:serde_json::Value=serde_json::from_str(include_str!("../tests/fixtures/mc-general-price-native.json")).unwrap();
        for row in fixture["rows"].as_array().unwrap() {
            let row=row.as_array().unwrap();
            let price=f64::from_bits(row[0].as_u64().unwrap());
            let unit=f64::from_bits(row[1].as_u64().unwrap());
            let step=row[2].as_u64().unwrap();let up=row[3].as_i64().unwrap()>0;
            assert_eq!(mc_native_price_units(price,unit,step,up).unwrap(),row[4].as_i64().unwrap(),"{row:?}");
        }
    }
    #[test]
    fn mc_defaults_and_product_price_grid_are_shared_across_strategies() {
        let mut serialized=serde_json::to_value(cfg()).unwrap();
        serialized.as_object_mut().unwrap().remove("price_rounding");
        serialized.as_object_mut().unwrap().remove("intrabar_policy");
        let mut c:Config=serde_json::from_value(serialized).unwrap();
        assert_eq!(c.price_rounding,PriceRounding::McNative);
        assert_eq!(c.intrabar_policy,IntrabarPolicy::NearestExtreme);
        for name in ["NH14","YAO1","KAIv7","其他策略"] {
            c.strategy_name=name.into();
            let o=Order{total:false,action:Action::BuyToCover,label:String::new(),quantity:None,kind:Kind::Stop,price:Some(16962.005)};
            assert_eq!(order_ticks(&o,&c).unwrap(),16962);
        }
        for (unit,step,tick,up_ticks,down_ticks) in [(0.01,25,0.25,5,4),(0.1,5,0.5,3,2),(1.0,5,5.0,2,1)] {
            c.price_scale=Some(1.0/unit);c.minimum_move=Some(step);c.tick_size=tick;
            assert!(c.validate().is_ok());
            let price=if tick==5.0 {6.2}else{1.231};
            let buy=Order{total:false,action:Action::Buy,label:String::new(),quantity:None,kind:Kind::Stop,price:Some(price)};
            assert_eq!(order_ticks(&buy,&c).unwrap(),up_ticks);
            assert_eq!(order_ticks(&Order{total:false,action:Action::Sell,..buy},&c).unwrap(),down_ticks);
        }
        let negative=Order{total:false,action:Action::Sell,label:String::new(),quantity:None,kind:Kind::Stop,price:Some(-1.2)};
        assert!(order_ticks(&negative,&c).is_err()); // 不可把原生非網格負價靜默改成另一個價格。
        c.price_scale=None;c.minimum_move=None;c.tick_size=0.25;
        assert!(c.validate().is_err());
    }
    #[test]
    fn equivalent_cutoff_formats_include_the_same_final_database_bar() {
        let conn=rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE bars(datetime TEXT); INSERT INTO bars VALUES ('2026-10-23T05:00:00.000000000'),('2026-10-23T05:00:00.000000001');").unwrap();
        for cutoff in ["2026-10-23T05:00:00","2026-10-23T05:00:00.0","2026-10-23T05:00:00.000000000"] {
            let count:i64=conn.query_row("SELECT count(*) FROM bars WHERE datetime<=?1",[database_cutoff(cutoff).unwrap()],|r|r.get(0)).unwrap();
            assert_eq!(count,1);
        }
        assert!(database_cutoff("不是日期").is_err());
        assert_eq!(database_cutoff("").unwrap(),"z");
    }
    #[test]
    fn entry_name_uses_filled_position_and_closed_entry_history() {
        let source="Vars: current_seen(0), prior_seen(0); if EntryName(0) = \"LE\" then current_seen=1; if EntryName(0) = \"SE\" and EntryName(1) = \"LE\" then prior_seen=1; if BarNumber=1 then Buy (\"LE\") next bar at market; if BarNumber=2 then SellShort (\"SE\") next bar at market;";
        let r=execute_source(source,&bars(),&cfg());
        assert_eq!(r.variable_observations["current_seen"].last,Some(1.0));
        assert_eq!(r.variable_observations["prior_seen"].last,Some(1.0));
        assert_eq!(r.open_trade.as_ref().unwrap().entry_label,"SE");
    }
    #[test]
    fn buy_ceil_sell_floor_covers_entry_exit_and_limit_stop() {
        let mut c=cfg();c.price_rounding=PriceRounding::BuyCeilSellFloor;
        c.tick_size=0.5;
        for action in [Action::Buy,Action::BuyToCover,Action::Sell,Action::SellShort] {
            for kind in [Kind::Limit,Kind::Stop] {
                for raw in [100.2,100.5] {
                    let order=Order {total:false,action,label:String::new(),quantity:None,kind,price:Some(raw)};
                    let expected=if raw==100.5 || buy_side(action) {201} else {200};
                    assert_eq!(order_ticks(&order,&c).unwrap(),expected);
                }
            }
        }
    }
    #[test]
    fn ceil_rounding_applies_to_both_sides_and_preserves_exact_tick() {
        let mut c=cfg();c.price_rounding=PriceRounding::Ceil;c.tick_size=0.5;
        for action in [Action::Buy,Action::BuyToCover,Action::Sell,Action::SellShort] {
            for kind in [Kind::Limit,Kind::Stop] {
                for price in [100.2,100.5] {
                    let order=Order {total:false,action,label:String::new(),quantity:None,kind,price:Some(price)};
                    assert_eq!(order_ticks(&order,&c).unwrap(),201);
                }
            }
        }
    }
    #[test]
    fn buy_floor_sell_ceil_covers_both_entry_and_exit_with_exact_ticks() {
        let mut c=cfg();c.price_rounding=PriceRounding::BuyFloorSellCeil;c.tick_size=0.5;
        for action in [Action::Buy,Action::BuyToCover,Action::Sell,Action::SellShort] {
            for kind in [Kind::Limit,Kind::Stop] {
                for price in [100.2,100.5,100.8] {
                    let order=Order {total:false,action,label:String::new(),quantity:None,kind,price:Some(price)};
                    let raw: f64=price/c.tick_size;
                    let expected=if buy_side(action) {raw.floor()} else {raw.ceil()};
                    assert_eq!(order_ticks(&order,&c).unwrap(),expected as i64);
                }
            }
        }
    }
    #[test]
    fn warmup_state_and_this_close() {
        let mut c = cfg();
        c.warmup_bars = 1;
        let r=execute_source("Vars: counter(0); counter = counter + 1; if counter = 1 then Buy next bar at market; if counter = 2 then Sell this bar on close;",&bars(),&c);
        assert_eq!(r.trades[0].entry_bar, 2);
        assert_eq!(r.trades[0].exit_bar, 2);
        assert_eq!(r.trades[0].exit_price, 132.0);
    }
    #[test]
    fn future_change_cannot_change_prior_fill() {
        let mut b = bars();
        let a = execute_source("if Close > 101 then Buy next bar at market;", &b, &cfg());
        b[2].close = 134.0;
        let z = execute_source("if Close > 101 then Buy next bar at market;", &b, &cfg());
        assert_eq!(a.fills[0].price, z.fills[0].price);
        assert_eq!(a.fills[0].bar, 1);
    }
    #[test]
    fn day_and_night_use_distinct_anchors() {
        let mut c = cfg();
        c.interval_minutes = 60;
        c.sessions = vec![
            Session {
                name: "day".into(),
                start_minute: 525,
                end_minute: 825,
            },
            Session {
                name: "night".into(),
                start_minute: 900,
                end_minute: 300,
            },
        ];
        c.validate().unwrap();
        let label = |s: &str| {
            chrono::DateTime::from_timestamp(
                session_bucket(parse_dt(s).unwrap(), &c).unwrap().unwrap() * 60,
                0,
            )
            .unwrap()
            .naive_utc()
            .format("%Y-%m-%dT%H:%M:%S")
            .to_string()
        };
        assert_eq!(label("2026-10-23T09:01:00"), "2026-10-23T09:45:00");
        assert_eq!(label("2026-10-23T19:01:00"), "2026-10-23T20:00:00");
        assert_eq!(label("2026-10-24T00:01:00"), "2026-10-24T01:00:00");
        assert!(session_bucket(parse_dt("2026-10-23T14:00:00").unwrap(), &c)
            .unwrap()
            .is_none());
    }
}
