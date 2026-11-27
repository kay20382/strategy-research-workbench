//! 商品主檔、交易時段、來源完整性與可追溯的Tick補建。無網路或券商依賴。
use crate::{canonical_market, database, tick_bars, tick_store};
use chrono::{Datelike, Duration, NaiveDate, NaiveDateTime, NaiveTime, Timelike};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest,Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, OpenOptions},
    path::Path,
};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TradingSession {
    pub name: String,
    pub open: String,
    pub close: String,
    /// 以開盤日計，1=週一至7=週日。
    pub weekdays: Vec<u32>,
    /// 0=開盤日，1=次交易日；遇週末／休市日向後移。
    pub trading_day_shift: u32,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Instrument {
    #[serde(default)]
    pub confirmed: bool,
    pub symbol: String,
    pub name: String,
    #[serde(default)]
    pub english_name: String,
    pub timezone: String,
    pub price_scale: f64,
    pub minimum_move: u64,
    pub point_value: f64,
    pub sessions: Vec<TradingSession>,
    /// 休市的開盤日期；不自動下載或猜測交易所假日。
    pub closed_dates: Vec<String>,
    pub trading_weekdays: Vec<u32>,
    pub auto_fill: bool,
}
fn sql<T>(r: rusqlite::Result<T>) -> Result<T, String> {
    r.map_err(|e| e.to_string())
}
fn parse_time(s: &str) -> Result<NaiveTime, String> {
    let t=NaiveTime::parse_from_str(s, "%H:%M:%S").or_else(|_|NaiveTime::parse_from_str(s,"%H:%M")).map_err(|_|format!("時間須為HH:mm:ss：{s}"))?;
    if t.second()!=0{return Err("商品時段目前須對齊整分鐘，秒請填00".into());}
    Ok(t)
}
fn parse_day(s: &str) -> Result<NaiveDate, String> {
    NaiveDate::parse_from_str(s, "%Y/%m/%d").or_else(|_|NaiveDate::parse_from_str(s,"%Y-%m-%d")).map_err(|_|format!("日期須為yyyy/MM/dd：{s}"))
}
fn parse_dt(s: &str) -> Result<NaiveDateTime, String> {
    NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S%.f")
        .or_else(|_| NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S%.f"))
        .or_else(|_| NaiveDateTime::parse_from_str(s, "%Y/%m/%d %H:%M:%S%.f"))
        .map_err(|_| format!("時間格式錯誤：{s}"))
}
fn iso(t: NaiveDateTime) -> String {
    t.format("%Y-%m-%dT%H:%M:%S%.9f").to_string()
}
pub fn defaults(symbol: &str) -> Instrument {
    let txf = symbol.eq_ignore_ascii_case("TXF1");
    Instrument {
        confirmed: txf,
        symbol: symbol.trim().to_uppercase(),
        name: symbol.into(),
        english_name: String::new(),
        timezone: "Asia/Taipei".into(),
        price_scale: 1.,
        minimum_move: 1,
        point_value: if txf { 200. } else { 1. },
        sessions: if txf {
            vec![
                TradingSession {
                    name: "日盤".into(),
                    open: "08:45".into(),
                    close: "13:45".into(),
                    weekdays: vec![1, 2, 3, 4, 5],
                    trading_day_shift: 0,
                },
                TradingSession {
                    name: "夜盤".into(),
                    open: "15:00".into(),
                    close: "05:00".into(),
                    weekdays: vec![1, 2, 3, 4, 5],
                    trading_day_shift: 1,
                },
            ]
        } else {
            vec![TradingSession {
                name: "全日".into(),
                open: "00:00".into(),
                close: "00:00".into(),
                weekdays: vec![1, 2, 3, 4, 5, 6, 7],
                trading_day_shift: 0,
            }]
        },
        closed_dates: vec![],
        trading_weekdays: if txf {
            vec![1, 2, 3, 4, 5]
        } else {
            vec![1, 2, 3, 4, 5, 6, 7]
        },
        auto_fill: false,
    }
}
fn validate(i: &Instrument) -> Result<(), String> {
    if i.symbol.is_empty()
        || i.symbol.len() > 64
        || !i
            .symbol
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "._-:".contains(c))
    {
        return Err("商品代碼限英數、點、底線、冒號與連字號，最多64字".into());
    }
    if i.name.trim().is_empty()
        || i.timezone.trim().is_empty()
        || !i.price_scale.is_finite()
        || i.price_scale <= 0.
        || i.minimum_move == 0
        || !i.point_value.is_finite()
        || i.point_value <= 0.
    {
        return Err("商品名稱、時區與價格規格無效".into());
    }
    if i.trading_weekdays.is_empty()
        || i.trading_weekdays.iter().any(|d| !(1..=7).contains(d))
        || i.sessions.is_empty()
    {
        return Err("至少設定一個交易日與時段".into());
    }
    let mut occupied = BTreeSet::new();
    for s in &i.sessions {
        let a = parse_time(&s.open)?.num_seconds_from_midnight() / 60;
        let b = parse_time(&s.close)?.num_seconds_from_midnight() / 60;
        if s.weekdays.is_empty()
            || s.weekdays.iter().any(|d| !(1..=7).contains(d))
            || s.trading_day_shift > 1
        {
            return Err("時段星期須為1至7，交易日位移限0或1".into());
        }
        let len = if a == b { 1440 } else { (b + 1440 - a) % 1440 };
        for day in &s.weekdays {
            for m in 0..len {
                let key = ((day - 1) * 1440 + a + m) % (7 * 1440);
                if !occupied.insert(key) {
                    return Err("交易時段重疊，請先修正".into());
                }
            }
        }
    }
    for d in &i.closed_dates {
        parse_day(d)?;
    }
    Ok(())
}
fn ensure(c: &Connection) -> Result<(), String> {
    sql(c.execute_batch("CREATE TABLE IF NOT EXISTS qm_instruments(symbol TEXT PRIMARY KEY COLLATE NOCASE,config TEXT NOT NULL,updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP);
      CREATE TABLE IF NOT EXISTS qm_events(id INTEGER PRIMARY KEY,symbol TEXT NOT NULL,action TEXT NOT NULL,details TEXT NOT NULL,created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP);
      CREATE TABLE IF NOT EXISTS qm_coverage_last(symbol TEXT PRIMARY KEY COLLATE NOCASE,signature TEXT NOT NULL,result TEXT NOT NULL);
      CREATE TABLE IF NOT EXISTS qm_derived(dataset_id INTEGER NOT NULL,time_key TEXT NOT NULL,tick_dataset_id INTEGER NOT NULL,tick_sha256 TEXT NOT NULL,profile TEXT NOT NULL,created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,PRIMARY KEY(dataset_id,time_key));"))
}
pub fn initialize(root: &Path, path: &Path) -> Result<(), String> {
    canonical_market::initialize(root, path)?;
    let c = database::open(path)?;
    ensure(&c)?;
    let mut stmt = sql(c.prepare("SELECT DISTINCT symbol FROM market_sources ORDER BY symbol"))?;
    let symbols = sql(stmt.query_map([], |r| r.get::<_, String>(0)))?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|e| e.to_string())?;
    for symbol in symbols {
        let i = defaults(&symbol);
        sql(c.execute(
            "INSERT OR IGNORE INTO qm_instruments(symbol,config) VALUES(?1,?2)",
            params![symbol, serde_json::to_string(&i).unwrap()],
        ))?;
    }
    Ok(())
}
pub fn save(path: &Path, instrument: &Instrument) -> Result<(), String> {
    let mut i = instrument.clone();
    i.confirmed = true;
    i.symbol = i.symbol.trim().to_uppercase();
    i.closed_dates=i.closed_dates.iter().map(|d|parse_day(d).map(|v|v.to_string())).collect::<Result<Vec<_>,_>>()?;
    for s in &mut i.sessions {s.open=parse_time(&s.open)?.format("%H:%M:%S").to_string();s.close=parse_time(&s.close)?.format("%H:%M:%S").to_string();}
    validate(&i)?;
    let mut c = database::open(path)?;
    ensure(&c)?;
    let tx = sql(c.transaction())?;
    let body = serde_json::to_string(&i).map_err(|e| e.to_string())?;
    sql(tx.execute("INSERT INTO qm_instruments(symbol,config) VALUES(?1,?2) ON CONFLICT(symbol) DO UPDATE SET config=excluded.config,updated_at=CURRENT_TIMESTAMP",params![i.symbol,body]))?;
    sql(tx.execute(
        "INSERT INTO qm_events(symbol,action,details) VALUES(?1,'save_instrument',?2)",
        params![i.symbol, body],
    ))?;
    sql(tx.commit())
}
pub fn get(path: &Path, symbol: &str) -> Result<Instrument, String> {
    let c = database::open_existing(path)?;
    let body: String = sql(c.query_row(
        "SELECT config FROM qm_instruments WHERE symbol=?1 COLLATE NOCASE",
        [symbol],
        |r| r.get(0),
    ))?;
    serde_json::from_str(&body).map_err(|e| e.to_string())
}
/// 有主檔才套用；舊任務與獨立研究資料庫仍沿用既有規則。
pub fn apply_to_task(
    path: &Path,
    symbol: &str,
    unit: &str,
    task: &mut Value,
) -> Result<(), String> {
    let c = database::open_existing(path)?;
    let exists: bool = sql(c.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='qm_instruments')",
        [],
        |r| r.get(0),
    ))?;
    if !exists {
        return Ok(());
    }
    let body: Option<String> = sql(c
        .query_row(
            "SELECT config FROM qm_instruments WHERE symbol=?1 COLLATE NOCASE",
            [symbol],
            |r| r.get(0),
        )
        .optional())?;
    if let Some(body) = body {
        let i: Instrument = serde_json::from_str(&body).map_err(|e| e.to_string())?;
        if !i.confirmed {
            return Ok(());
        }
        validate(&i)?;
        task["parameters"]["price_scale"] = json!(i.price_scale);
        task["parameters"]["minimum_move"] = json!(i.minimum_move);
        task["parameters"]["tick_size"] = json!(i.minimum_move as f64 / i.price_scale);
        task["parameters"]["point_value"] = json!(i.point_value);
        if unit != "days" {
            task["parameters"]["sessions"]=json!(i.sessions.iter().map(|s|{let a=parse_time(&s.open).unwrap();let b=parse_time(&s.close).unwrap();json!({"name":s.name,"start_minute":a.hour()*60+a.minute(),"end_minute":b.hour()*60+b.minute()})}).collect::<Vec<_>>());
            // 核心Session不接受同開收時間，24小時商品以空時段代表不過濾。
            if i.sessions.len() == 1 && i.sessions[0].open == i.sessions[0].close {
                task["parameters"]["sessions"] = json!([]);
            }
        }
        task["market"]["instrument_profile"] = json!(i);
    }
    Ok(())
}
pub fn inventory(path: &Path) -> Result<Value, String> {
    let c = database::open_existing(path)?;
    let mut s = sql(c.prepare("SELECT config FROM qm_instruments ORDER BY symbol"))?;
    let instruments = sql(s.query_map([], |r| r.get::<_, String>(0)))?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|e| e.to_string())?;
    let sources = canonical_market::sources(path)?;
    let mut rows = vec![];
    for body in instruments {
        let i: Instrument = serde_json::from_str(&body).map_err(|e| e.to_string())?;
        let mut available = sources
            .as_array()
            .unwrap()
            .iter()
            .filter(|r| {
                r["symbol"]
                    .as_str()
                    .is_some_and(|s| s.eq_ignore_ascii_case(&i.symbol))
            })
            .cloned()
            .collect::<Vec<_>>();
        for s in &mut available {
            let id = s["dataset_id"].as_i64().unwrap();
            let (derived,stale):(i64,i64)=sql(c.query_row("SELECT COUNT(*),COALESCE(SUM(CASE WHEN q.tick_sha256<>COALESCE(d.sha256,'') THEN 1 ELSE 0 END),0) FROM qm_derived q LEFT JOIN datasets d ON d.id=q.tick_dataset_id WHERE q.dataset_id=?1",[id],|r|Ok((r.get(0)?,r.get(1)?))))?;
            s["tick_derived_rows"] = json!(derived);
            s["tick_source_changed_rows"] = json!(stale);
        }
        rows.push(json!({"last_coverage":last_coverage(&c,&i.symbol)?,"instrument":i,"sources":available}));
    }
    Ok(json!(rows))
}
fn deletion_plan(c:&Connection,symbol:&str)->Result<Value,String>{
    let body:String=sql(c.query_row("SELECT config FROM qm_instruments WHERE symbol=?1 COLLATE NOCASE",[symbol],|r|r.get(0)))?;
    let instrument:Instrument=serde_json::from_str(&body).map_err(|e|e.to_string())?;
    let mut stmt=sql(c.prepare("SELECT id,kind,row_count,COALESCE(sha256,''),COALESCE(first_datetime,''),COALESCE(last_datetime,''),path FROM datasets WHERE symbol=?1 COLLATE NOCASE ORDER BY id"))?;
    let data=sql(stmt.query_map([&instrument.symbol],|r|Ok(json!({"id":r.get::<_,i64>(0)?,"kind":r.get::<_,String>(1)?,"rows":r.get::<_,i64>(2)?,"sha256":r.get::<_,String>(3)?,"first":r.get::<_,String>(4)?,"last":r.get::<_,String>(5)?,"source_path":r.get::<_,String>(6)?}))))?.collect::<rusqlite::Result<Vec<_>>>().map_err(|e|e.to_string())?;
    let token=format!("{:x}",Sha256::digest(serde_json::to_vec(&json!([&body,&data])).unwrap()));
    Ok(json!({"symbol":instrument.symbol,"name":instrument.name,"datasets":data,"token":token,"source_files_deleted":false}))
}
pub fn delete_plan(path:&Path,symbol:&str)->Result<Value,String>{deletion_plan(&database::open_existing(path)?,symbol)}
/// 必須重送畫面確認的商品、資料清單簽章及刪行情旗標；任何變動均拒絕舊確認。
pub fn delete_instrument(path:&Path,symbol:&str,confirmation:&str,token:&str,delete_data:bool)->Result<Value,String>{
    if confirmation.trim()!=symbol{return Err("確認欄請完整輸入商品代碼，未刪除".into());}
    let mut c=database::open(path)?;ensure(&c)?;
    let tx=sql(c.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate))?;
    let plan=deletion_plan(&tx,symbol)?;
    if plan["token"]!=token{return Err("商品或行情已變更，請重新檢視刪除清單後再確認".into());}
    let datasets=plan["datasets"].as_array().unwrap();
    if !datasets.is_empty()&&!delete_data{return Err("商品仍有行情，請先匯出所需資料，再勾選連同行情刪除；目前未刪除".into());}
    for dataset in datasets {
        let id=dataset["id"].as_i64().unwrap();
        let other:i64=sql(tx.query_row("SELECT COUNT(*) FROM market_sources WHERE dataset_id=?1 AND symbol<>?2 COLLATE NOCASE",params![id,symbol],|r|r.get(0)))?;
        if other!=0{return Err("資料集被其他商品引用，未刪除，請先檢查資料關聯".into());}
        sql(tx.execute("DELETE FROM qm_derived WHERE dataset_id=?1 OR tick_dataset_id=?1",[id]))?;
        for table in ["records","market_bar_keys","market_sources","tick_blocks","tick_imports","tick_canonical","dataset_storage"] {
            let exists:bool=sql(tx.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1)",[table],|r|r.get(0)))?;
            if exists {sql(tx.execute(&format!("DELETE FROM {table} WHERE dataset_id=?1"),[id]))?;}
        }
        sql(tx.execute("DELETE FROM datasets WHERE id=?1",[id]))?;
    }
    sql(tx.execute("DELETE FROM qm_instruments WHERE symbol=?1 COLLATE NOCASE",[symbol]))?;
    sql(tx.execute("DELETE FROM qm_coverage_last WHERE symbol=?1 COLLATE NOCASE",[symbol]))?;
    sql(tx.execute("INSERT INTO qm_events(symbol,action,details) VALUES(?1,'delete_instrument',?2)",params![symbol,plan.to_string()]))?;
    sql(tx.commit())?;
    Ok(json!({"deleted":true,"symbol":symbol,"datasets_deleted":datasets.len(),"source_files_deleted":false,"note":"商品與所選行情已永久刪除；保留匯入原檔、既有報表及刪除紀錄。SQLite檔案空間供後續重用，不自動壓縮。"}))
}
/// 使用者明示來源既有時鐘的意義，不對原始時間做時區轉換。
pub fn annotate_source(
    path: &Path,
    symbol: &str,
    kind: &str,
    timezone: &str,
    role: &str,
) -> Result<Value, String> {
    let _ = get(path, symbol)?;
    if timezone.trim().is_empty()
        || (kind == "tick" && role != "event")
        || (kind != "tick" && !["end", "start", "unspecified"].contains(&role))
    {
        return Err("來源時區／時間標記不合法".into());
    }
    let mut c = database::open(path)?;
    let tx = sql(c.transaction())?;
    let (id, old_zone, old_role, _) = source(&tx, symbol, kind)?.ok_or("此商品尚無指定來源")?;
    if old_role != "unspecified" && old_role != role {
        return Err("已知起訖標記不能直接改寫；需另行轉換時間並驗證，避免把開盤當收盤".into());
    }
    sql(tx.execute(
        "UPDATE datasets SET timezone=?1,timestamp_role=?2 WHERE id=?3",
        params![timezone, role, id],
    ))?;
    let receipt = json!({"symbol":symbol,"kind":kind,"previous_timezone":old_zone,"previous_role":old_role,"timezone":timezone,"role":role,"timestamps_unchanged":true});
    sql(tx.execute(
        "INSERT INTO qm_events(symbol,action,details) VALUES(?1,'annotate_source',?2)",
        params![symbol, receipt.to_string()],
    ))?;
    sql(tx.commit())?;
    Ok(receipt)
}
#[derive(Clone)]
struct Window {
    start: NaiveDateTime,
    end: NaiveDateTime,
    day: NaiveDate,
}
fn windows(i: &Instrument, from: NaiveDate, to: NaiveDate) -> Result<Vec<Window>, String> {
    if to < from {
        return Err("起迄日期不可倒置".into());
    }
    let mut result = vec![];
    let mut date = from - Duration::days(8);
    while date <= to {
        for s in &i.sessions {
            if !s.weekdays.contains(&date.weekday().number_from_monday())
                || i.closed_dates.contains(&date.to_string())
            {
                continue;
            }
            let start = date.and_time(parse_time(&s.open)?);
            let close = parse_time(&s.close)?;
            let end = (if close <= start.time() {
                date + Duration::days(1)
            } else {
                date
            })
            .and_time(close);
            let mut day = date + Duration::days(s.trading_day_shift as i64);
            if s.trading_day_shift > 0 {
                let mut n = 0;
                while !i
                    .trading_weekdays
                    .contains(&day.weekday().number_from_monday())
                    || i.closed_dates.contains(&day.to_string())
                {
                    day += Duration::days(1);
                    n += 1;
                    if n > 370 {
                        return Err("交易日曆找不到下一交易日".into());
                    }
                }
            }
            if (end.date() >= from && start.date() <= to) || (day >= from && day <= to) {
                result.push(Window { start, end, day });
            }
        }
        date += Duration::days(1);
    }
    result.sort_by_key(|w| w.start);
    Ok(result)
}

pub struct TradingCalendar(Vec<Window>);

impl TradingCalendar {
    pub fn new(instrument: &Instrument, from: NaiveDate, to: NaiveDate) -> Result<Self, String> {
        validate(instrument)?;
        windows(instrument, from, to).map(Self)
    }

    pub fn trading_date(&self, time: &str, close_label: bool) -> Option<NaiveDate> {
        let time = parse_dt(time).ok()?;
        if close_label {
            let at = self.0.partition_point(|window| window.start < time);
            if let Some(window) = at.checked_sub(1).and_then(|i| self.0.get(i)) {
                if window.end == time { return Some(window.day); }
            }
        }
        containing(&self.0, time).map(|window| window.day)
    }
}
fn range(from: &str, to: &str) -> Result<(NaiveDateTime, NaiveDateTime), String> {
    Ok((
        parse_day(from)?.and_hms_opt(0, 0, 0).unwrap(),
        parse_day(to)?.and_hms_opt(23, 59, 59).unwrap() + Duration::nanoseconds(999_999_999),
    ))
}
fn source(
    c: &Connection,
    symbol: &str,
    kind: &str,
) -> Result<Option<(i64, String, String, String)>, String> {
    sql(c.query_row("SELECT d.id,d.timezone,d.timestamp_role,COALESCE(d.sha256,'') FROM market_sources m JOIN datasets d ON d.id=m.dataset_id WHERE m.symbol=?1 COLLATE NOCASE AND m.kind=?2",params![symbol,kind],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional())
}
fn scan_ticks(
    c: &Connection,
    id: i64,
    lo: NaiveDateTime,
    hi: NaiveDateTime,
    mut f: impl FnMut(NaiveDateTime, i64, i64) -> Result<(), String>,
) -> Result<usize, String> {
    tick_store::visit(c, id, &iso(lo), &iso(hi), |ns, p, v| {
        let time = chrono::DateTime::from_timestamp(
            ns.div_euclid(1_000_000_000),
            ns.rem_euclid(1_000_000_000) as u32,
        )
        .ok_or("Tick時間超限")?
        .naive_utc();
        f(time, p, v)
    })
}
fn minute_key(t: NaiveDateTime) -> NaiveDateTime {
    t.with_second(0).unwrap().with_nanosecond(0).unwrap() + Duration::minutes(1)
}
fn containing(ws: &[Window], t: NaiveDateTime) -> Option<&Window> {
    let at = ws.partition_point(|w| w.start <= t);
    at.checked_sub(1)
        .and_then(|n| ws.get(n))
        .filter(|w| t < w.end)
}
fn occupied_bars(
    c: &Connection,
    id: i64,
    kind: &str,
    role: &str,
    lo: NaiveDateTime,
    hi: NaiveDateTime,
) -> Result<BTreeSet<String>, String> {
    let mut stmt=sql(c.prepare("SELECT datetime FROM records WHERE dataset_id=?1 AND datetime>=?2 AND datetime<=?3 ORDER BY datetime,ordinal"))?;
    let mut keys = BTreeSet::new();
    for row in sql(stmt.query_map(
        params![id, lo.date().to_string(), format!("{}~", hi.date())],
        |r| r.get::<_, String>(0),
    ))? {
        let value = row.map_err(|e| e.to_string())?;
        if kind == "daily" {
            keys.insert(value[..10].to_owned());
        } else {
            let mut t = parse_dt(&value)?;
            if role == "start" {
                t += Duration::minutes(1);
            }
            keys.insert(iso(t));
        }
    }
    Ok(keys)
}
fn source_date_bounds(path:&Path,symbol:&str)->Result<(NaiveDate,NaiveDate),String>{
    let c=database::open_existing(path)?;
    let mut stmt=sql(c.prepare("SELECT d.first_datetime,d.last_datetime FROM market_sources m JOIN datasets d ON d.id=m.dataset_id WHERE m.symbol=?1 COLLATE NOCASE AND m.kind IN ('tick','minute','daily') AND d.row_count>0"))?;
    let bounds=sql(stmt.query_map([symbol],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?))))?.collect::<rusqlite::Result<Vec<_>>>().map_err(|e|e.to_string())?;
    let mut first=None;let mut last=None;
    for (a,b) in bounds {
        let a=parse_day(a.get(..10).ok_or("來源起始日期無效")?)?;
        let b=parse_day(b.get(..10).ok_or("來源結束日期無效")?)?;
        first=Some(first.map_or(a,|old:NaiveDate|old.min(a)));last=Some(last.map_or(b,|old:NaiveDate|old.max(b)));
    }
    let (Some(a),Some(b))=(first,last)else{return Err("此商品三種來源皆無行情，尚無可檢查範圍".into())};
    Ok((a,b))
}
pub fn coverage_all(path:&Path,symbol:&str)->Result<Value,String>{
    let (a,b)=source_date_bounds(path,symbol)?;
    coverage(path,symbol,&a.to_string(),&b.to_string())
}
pub fn fill_all(path:&Path,symbol:&str)->Result<Value,String>{
    let (mut start,end)=source_date_bounds(path,symbol)?;
    let first=start;let mut completed=0;let mut totals=BTreeMap::<String,u64>::new();
    while start<=end{
        let last=(start+Duration::days(366)).min(end);
        match fill_from_ticks(path,symbol,&start.to_string(),&last.to_string()){
            Ok(value)=>{
                if let Some(inserted)=value["inserted"].as_object(){for (key,n) in inserted{*totals.entry(key.clone()).or_default()+=n.as_u64().unwrap_or(0);}}
                completed+=1;
            }
            Err(e)=>{
                if completed==0{return Err(e);}
                return Ok(json!({"status":"partial","inserted":totals,"completed_chunks":completed,"warning":format!("前{completed}段已提交；{}～{}補建失敗：{e}。請重新檢查後再執行補建。",start,last)}));
            }
        }
        start=last+Duration::days(1);
    }
    Ok(json!({"status":"completed","from":first.to_string(),"to":end.to_string(),"inserted":totals,"completed_chunks":completed,"warning":"已按三來源完整期間分段補建，只填缺少時間，未覆寫既有K棒；無Tick行情不補造。"}))
}
pub fn coverage(path: &Path, symbol: &str, from: &str, to: &str) -> Result<Value, String> {
    let c=database::open(path)?;
    let signature=coverage_signature(&c,symbol)?;
    let (lo,hi)=range(from,to)?;
    if hi<lo{return Err("起迄日期不可倒置".into());}
    let mut start=lo.date();let mut result:Option<Value>=None;
    // 不限制總期間；每段七日，避免建立多年逐分鐘字串集合。
    while start<=hi.date(){
        let end=(start+Duration::days(6)).min(hi.date());
        let part=compute_coverage(path,symbol,&start.to_string(),&end.to_string())?;
        if let Some(all)=result.as_mut(){
            all["to"]=part["to"].clone();all["timeline_to_ms"]=part["timeline_to_ms"].clone();
            for k in 0..3 {
                let dst=&mut all["sources"][k];let src=&part["sources"][k];
                for key in ["expected","observed","missing","tick_count"]{dst[key]=json!(dst[key].as_u64().unwrap()+src[key].as_u64().unwrap());}
                dst["missing_seconds"]=json!(dst["missing_seconds"].as_f64().unwrap()+src["missing_seconds"].as_f64().unwrap());
                for segment in src["timeline"].as_array().unwrap(){
                    let timeline=dst["timeline"].as_array_mut().unwrap();
                    if let Some(last)=timeline.last_mut(){if last["end_ms"]==segment["start_ms"]&&last["status"]==segment["status"]{last["end_ms"]=segment["end_ms"].clone();continue;}}
                    timeline.push(segment.clone());
                }
                for gap in src["gaps"].as_array().unwrap(){
                    let gaps=dst["gaps"].as_array_mut().unwrap();
                    if let Some(last)=gaps.last_mut(){
                        let joins=if k==0{last["to"]==gap["from"]&&last["anchor_from"]==gap["anchor_from"]&&last["anchor_to"]==gap["anchor_to"]}else if k==1{parse_dt(gap["from"].as_str().unwrap())?-parse_dt(last["to"].as_str().unwrap())?==Duration::minutes(1)}else{false};
                        if joins {
                            last["to"]=gap["to"].clone();
                            if k==0{last["duration_seconds"]=json!(last["duration_seconds"].as_f64().unwrap()+gap["duration_seconds"].as_f64().unwrap());}
                            else{last["count"]=json!(last["count"].as_u64().unwrap()+gap["count"].as_u64().unwrap());}continue;
                        }
                    }
                    gaps.push(gap.clone());
                }
            }
        }else{result=Some(part);}
        start=end+Duration::days(1);
    }
    let mut result=result.unwrap();result["from"]=json!(from);result["to"]=json!(to);
    for k in 0..3{let row=&mut result["sources"][k];let count=row["gaps"].as_array().unwrap().len();row["gap_count"]=json!(count);if k==0{row["missing"]=json!(count);}row["gaps"].as_array_mut().unwrap().truncate(200);}
    let elapsed=std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_err(|e|e.to_string())?;
    let now=chrono::DateTime::from_timestamp(elapsed.as_secs() as i64,elapsed.subsec_nanos()).ok_or("系統時間超出範圍")?;
    result["checked_at"]=json!(now.with_timezone(&chrono::FixedOffset::east_opt(8*3600).unwrap()).format("%Y/%m/%d %H:%M:%S").to_string());
    result["checked_timezone"]=json!("Asia/Taipei");
    result["stale"]=json!(signature!=coverage_signature(&c,symbol)?);
    sql(c.execute("INSERT INTO qm_coverage_last(symbol,signature,result) VALUES(?1,?2,?3) ON CONFLICT(symbol) DO UPDATE SET signature=excluded.signature,result=excluded.result",params![symbol,signature,result.to_string()]))?;
    Ok(result)
}
fn coverage_signature(c:&Connection,symbol:&str)->Result<String,String>{
    let revision:i64=sql(c.query_row("SELECT COALESCE(MAX(id),0) FROM qm_events WHERE symbol=?1 COLLATE NOCASE",[symbol],|r|r.get(0)))?;
    Ok(format!("v2-unlimited-3min-{}-{revision}",deletion_plan(c,symbol)?["token"].as_str().unwrap()))
}
fn last_coverage(c:&Connection,symbol:&str)->Result<Value,String>{
    let saved:Option<(String,String)>=sql(c.query_row("SELECT signature,result FROM qm_coverage_last WHERE symbol=?1 COLLATE NOCASE",[symbol],|r|Ok((r.get(0)?,r.get(1)?))).optional())?;
    let Some((signature,body))=saved else{return Ok(Value::Null)};
    let mut result:Value=serde_json::from_str(&body).map_err(|e|format!("檢查紀錄讀取失敗：{e}"))?;
    result["stale"]=json!(signature!=coverage_signature(c,symbol)?);
    Ok(result)
}

/// 唯讀擷取最後一次完整性檢查，避免重新掃描行情。
pub fn backtest_coverage(path: &Path, report: &Value) -> Value {
    let provenance = &report["provenance"];
    let mut summary = json!({
        "database": path, "symbol": provenance["symbol"], "kind": provenance["kind"],
        "dataset_id": provenance["dataset_id"], "source": provenance["source"],
        "source_sha256": provenance["source_sha256"], "timezone": provenance["timezone"],
        "source_timestamp_role": provenance["source_timestamp_role"],
        "first_bar": report["first_bar"], "last_bar": report["last_bar"],
        "source_rows": provenance["source_rows"], "partial_bars": provenance["partial_bars"],
        "status": "not_checked",
    });
    if let Err(reason) = attach_coverage(path, &mut summary) {
        summary["status"] = json!("unavailable");
        summary["error"] = json!(reason);
    }
    summary
}

fn attach_coverage(path: &Path, summary: &mut Value) -> Result<(), String> {
    let db = database::open_existing(path)?;
    let tx = sql(db.unchecked_transaction())?;
    let exists: bool = sql(tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='qm_coverage_last')",
        [], |r| r.get(0),
    ))?;
    if !exists { return Ok(()); }
    let symbol = summary["symbol"].as_str().ok_or("回測缺少商品代碼")?;
    let kind = match summary["kind"].as_str().ok_or("回測缺少行情類型")? {
        "trade" => "tick".to_owned(),
        kind => kind.to_owned(),
    };
    let saved = last_coverage(&tx, symbol)?;
    if saved.is_null() { return Ok(()); }
    let canonical = source(&tx, symbol, &kind)?;
    let source_matches = canonical.is_some_and(|(id, zone, role, hash)| {
        summary["dataset_id"] == id && summary["timezone"] == zone
            && summary["source_timestamp_role"] == role && summary["source_sha256"] == hash
    });
    let row = saved["sources"].as_array().and_then(|rows| rows.iter().find(|row| row["kind"] == kind));
    let first = parse_dt(summary["first_bar"].as_str().ok_or("回測缺少起點")?)?;
    let last = parse_dt(summary["last_bar"].as_str().ok_or("回測缺少終點")?)?;
    let from = parse_day(saved["from"].as_str().ok_or("檢查缺少起日")?)?;
    let to = parse_day(saved["to"].as_str().ok_or("檢查缺少迄日")?)?;
    let covered = first.date() >= from && last.date() <= to;
    let overlaps = last.date() >= from && first.date() <= to;
    let stale = saved["stale"] == true;
    summary["status"] = json!(if !source_matches || row.is_none_or(|r| r["available"] != true) {
        "source_mismatch"
    } else if stale { "stale" } else if !overlaps { "out_of_range" } else if !covered {
        "partial_range"
    } else { "checked" });
    summary["last_check"] = json!({"from":saved["from"],"to":saved["to"],
        "checked_at":saved["checked_at"],"timezone":saved["checked_timezone"],
        "stale":stale,"source_matches":source_matches,"covers_run":covered,
        "calendar":saved["calendar"]});
    if let Some(row) = row.filter(|r| source_matches && r["available"] == true) {
        let gap_date = |v: &Value| v.as_str().and_then(|s| s.get(..10)).and_then(|s| parse_day(s).ok());
        let within = |g: &&Value| {
            if kind == "daily" {
                return gap_date(&g["from"]).is_some_and(|d| d <= last.date())
                    && gap_date(&g["to"]).is_some_and(|d| d >= first.date());
            }
            g["from"].as_str().and_then(|s| parse_dt(s).ok()).is_some_and(|t| t <= last)
                && g["to"].as_str().and_then(|s| parse_dt(s).ok()).is_some_and(|t| t >= first)
        };
        let gaps = row["gaps"].as_array().ok_or("檢查缺少缺口明細")?;
        let examples: Vec<_> = gaps.iter().filter(within).take(10).collect();
        summary["last_check"]["source_totals"] = json!({"missing":row["missing"],
            "missing_unit":row["missing_unit"],"gap_count":row["gap_count"],
            "meaning":row["meaning"],"warning":row["warning"]});
        summary["overlapping_gap_examples"] = json!(examples);
        summary["gap_examples_truncated"] = json!(row["gap_count"].as_u64().unwrap_or(0) > gaps.len() as u64
            || examples.len() == 10 && gaps.iter().filter(within).nth(10).is_some());
    }
    Ok(())
}
fn compute_coverage(path: &Path, symbol: &str, from: &str, to: &str) -> Result<Value, String> {
    let i = get(path, symbol)?;
    let (lo, hi) = range(from, to)?;
    let ws = windows(&i, lo.date(), hi.date())?;
    let c = database::open_existing(path)?;
    let mut expected = BTreeSet::new();
    let mut days = BTreeSet::new();
    for w in &ws {
        let mut t = w.start + Duration::minutes(1);
        while t <= w.end {
            if t >= lo && t <= hi {
                expected.insert(iso(t));
                if w.day >= lo.date() && w.day <= hi.date() {
                    days.insert(w.day.to_string());
                }
            }
            t += Duration::minutes(1);
        }
    }
    let mut rows = vec![];
    for kind in ["tick", "minute", "daily"] {
        let info = source(&c, symbol, kind)?;
        let mut present = BTreeSet::new();
        let mut tick_count = 0;
        let mut timeline:Vec<Value>=vec![];
        let mut tick_gaps:Vec<Value>=vec![];
        let mut missing_seconds=0.0;
        let mut session_ticks:BTreeMap<NaiveDateTime,(NaiveDateTime,NaiveDateTime)>=BTreeMap::new();
        if let Some((id, _, role, _)) = &info {
            if kind == "tick" {
                // 讀取查詢邊界所屬完整時段，才能判斷跨查詢邊界的相鄰成交。
                let relevant=ws.iter().filter(|w|w.end>lo&&w.start<=hi).collect::<Vec<_>>();
                let scan_lo=relevant.iter().map(|w|w.start).min().unwrap_or(lo).min(lo-Duration::minutes(1));
                let scan_hi=relevant.iter().map(|w|w.end).max().unwrap_or(hi);
                scan_ticks(&c, *id, scan_lo, scan_hi, |t, _, _| {
                    if t>=lo&&t<=hi{tick_count+=1;}
                    let key = minute_key(t);
                    if let Some(w)=containing(&ws,t){
                        if key>=lo&&key<=hi {present.insert(iso(key));}
                        if let Some((_,previous))=session_ticks.get_mut(&w.start) {
                            let a=(*previous).max(lo);let b=t.min(hi+Duration::nanoseconds(1));
                            if b>a {
                                let missing=t-*previous>=Duration::seconds(180);
                                push_timeline(&mut timeline,a,b,if missing{"missing"}else{"present"});
                                if missing {
                                    let seconds=(b-a).num_milliseconds() as f64/1000.;missing_seconds+=seconds;
                                    tick_gaps.push(json!({"from":iso(a),"to":iso(b),"anchor_from":iso(*previous),"anchor_to":iso(t),"count":1,"duration_seconds":seconds}));
                                }
                            }
                            *previous=t;
                        } else {session_ticks.insert(w.start,(t,t));}
                    }
                    Ok(())
                })?;
            } else {
                present = occupied_bars(&c, *id, kind, role, lo, hi)?;
            }
        }
        if kind=="tick" {
            for w in &ws {
                if w.end<=lo || w.start>hi {continue;}
                let edges=if let Some((first,last))=session_ticks.get(&w.start){vec![(w.start,*first),(*last,w.end)]}else{vec![(w.start,w.end)]};
                for (start,end) in edges {
                    let missing=!session_ticks.contains_key(&w.start) || end-start>=Duration::seconds(180);
                    let a=start.max(lo);let b=end.min(hi+Duration::nanoseconds(1));
                    if b<=a {continue;}
                    push_timeline(&mut timeline,a,b,if missing{"missing"}else{"present"});
                    if missing {
                        let seconds=(b-a).num_milliseconds() as f64/1000.;missing_seconds+=seconds;
                        tick_gaps.push(json!({"from":iso(a),"to":iso(b),"anchor_from":iso(start),"anchor_to":iso(end),"count":1,"duration_seconds":seconds}));
                    }
                }
            }
            timeline.sort_by_key(|v|v["start_ms"].as_i64().unwrap());
            tick_gaps.sort_by(|a,b|a["from"].as_str().cmp(&b["from"].as_str()));
        }
        let wanted = if kind == "daily" { &days } else { &expected };
        let missing = if kind=="tick"{vec![]}else{wanted.difference(&present).cloned().collect::<Vec<_>>()};
        if kind!="tick" {
            for key in wanted {
                let (start,end)=if kind=="daily" {let t=parse_day(key)?.and_hms_opt(0,0,0).unwrap();(t,t+Duration::days(1))}
                    else{let t=parse_dt(key)?;(t-Duration::minutes(1),t)};
                let a=start.max(lo);let b=end.min(hi+Duration::nanoseconds(1));
                if b>a {push_timeline(&mut timeline,a,b,if present.contains(key){"present"}else{"missing"});}
            }
        }
        // 壓縮相連缺漏分鐘，保留完整計數；UI最多列200區段。
        let mut gaps: Vec<Value> = vec![];
        let mut start: Option<String> = None;
        let mut previous = String::new();
        let mut n = 0;
        for value in &missing {
            let adjacent = kind != "daily"
                && !previous.is_empty()
                && parse_dt(value)? - parse_dt(&previous)? == Duration::minutes(1);
            if !adjacent {
                if let Some(a) = start.take() {
                    gaps.push(json!({"from":a,"to":previous,"count":n}));
                }
                start = Some(value.clone());
                n = 0;
            }
            previous = value.clone();
            n += 1;
        }
        if let Some(a) = start {
            gaps.push(json!({"from":a,"to":previous,"count":n}));
        }
        if kind=="tick" {gaps=tick_gaps;}
        let gap_count = gaps.len();
        rows.push(json!({"kind":kind,"available":info.is_some(),"expected":wanted.len(),"observed":wanted.intersection(&present).count(),"missing":if kind=="tick"{gap_count}else{missing.len()},"gap_count":gap_count,"gaps":gaps,"tick_count":tick_count,"timeline":timeline,"missing_seconds":missing_seconds,"missing_unit":if kind=="tick"{"相鄰成交缺口段數"}else{"棒數"},
            "meaning":if kind=="tick"{"交易時段內相鄰Tick、開盤至首筆或末筆至收盤空白至少3分鐘，標記疑似缺漏；整個時段無Tick則整段標紅。白色為有資料或未達3分鐘門檻，休盤不畫資料色塊。無成交不等同已證實漏收。"}else{"依商品星期／時段／手動休市日推算的預期資料；不補造休市行情"},
            "timestamp_assumption":info.as_ref().map(|s|s.2.as_str()).unwrap_or("無來源"),"warning":if info.as_ref().is_some_and(|x|x.2=="unspecified"){"來源時間標記未知，本次按商品K棒結束時間假設；未改原資料"}else{""}}));
    }
    Ok(
        json!({"symbol":symbol,"from":from,"to":to,"timeline_from_ms":lo.and_utc().timestamp_millis(),"timeline_to_ms":(hi+Duration::nanoseconds(1)).and_utc().timestamp_millis(),"tick_gap_threshold_seconds":180,"calendar":"依手動時段與休市日，未接交易所假日服務","sources":rows}),
    )
}
fn push_timeline(items:&mut Vec<Value>,start:NaiveDateTime,end:NaiveDateTime,status:&str){
    let a=start.and_utc().timestamp_millis();let b=end.and_utc().timestamp_millis();
    if let Some(last)=items.last_mut(){if last["status"]==status&&last["end_ms"]==a {last["end_ms"]=json!(b);return;}}
    items.push(json!({"start_ms":a,"end_ms":b,"status":status}));
}
#[derive(Clone)]
struct Aggregate {
    o: i64,
    h: i64,
    l: i64,
    c: i64,
    v: i128,
    count: usize,
}
impl Aggregate {
    fn add(&mut self, p: i64, v: i64) -> Result<(), String> {
        self.h = self.h.max(p);
        self.l = self.l.min(p);
        self.c = p;
        self.v = self.v.checked_add(v as i128).ok_or("成交量溢位")?;
        self.count += 1;
        Ok(())
    }
    fn new(p: i64, v: i64) -> Self {
        Self {
            o: p,
            h: p,
            l: p,
            c: p,
            v: v as i128,
            count: 1,
        }
    }
    fn payload(&self) -> Value {
        json!({"type":"bar","open":tick_bars::text(self.o as i128),"high":tick_bars::text(self.h as i128),"low":tick_bars::text(self.l as i128),"close":tick_bars::text(self.c as i128),"volume":tick_bars::text(self.v)})
    }
}
pub fn fill_from_ticks(path: &Path, symbol: &str, from: &str, to: &str) -> Result<Value, String> {
    let i = get(path, symbol)?;
    if !i.confirmed {
        return Err("此商品主檔仍為預設範本，請確認價格規格／交易時段後按儲存，再執行補建".into());
    }
    let (lo, hi) = range(from, to)?;
    if (hi.date()-lo.date()).num_days()>366{return Err("補建每次最多367日；完整性檢查不限制天數".into());}
    let ws = windows(&i, lo.date(), hi.date())?;
    let mut c = database::open(path)?;
    ensure(&c)?;
    // 整次補建持有同一資料庫交易，Tick與既有K棒版本不可在中間改變。
    let tx = sql(c.transaction())?;
    let (tick_id, zone, _, hash) =
        source(&tx, symbol, "tick")?.ok_or("此商品沒有Tick來源，無法補建")?;
    let complete_days = ws
        .iter()
        .filter(|w| w.day >= lo.date() && w.day <= hi.date())
        .map(|w| w.day)
        .filter(|day| {
            ws.iter()
                .filter(|w| w.day == *day)
                .all(|w| w.start >= lo && w.end <= hi + Duration::nanoseconds(1))
        })
        .collect::<BTreeSet<_>>();
    let mut minute: BTreeMap<String, Aggregate> = BTreeMap::new();
    let mut daily: BTreeMap<String, Aggregate> = BTreeMap::new();
    let count = scan_ticks(&tx, tick_id, lo - Duration::minutes(1), hi, |time, p, v| {
        if let Some(w) = containing(&ws, time) {
            let end = minute_key(time);
            if end >= lo && end <= hi {
                let key = iso(end);
                if let Some(a) = minute.get_mut(&key) {
                    a.add(p, v)?;
                } else {
                    minute.insert(key, Aggregate::new(p, v));
                }
            }
            // 日期完整範圍以當日所有設定時段計，截到前一日夜盤的日期不產日K。
            if complete_days.contains(&w.day) {
                let key = w.day.to_string();
                if let Some(a) = daily.get_mut(&key) {
                    a.add(p, v)?;
                } else {
                    daily.insert(key, Aggregate::new(p, v));
                }
            }
        }
        Ok(())
    })?;
    let profile = serde_json::to_string(&i).map_err(|e| e.to_string())?;
    let mut added = BTreeMap::new();
    for (kind, bars) in [("minute", minute), ("daily", daily)] {
        if bars.is_empty() {
            added.insert(kind, 0);
            continue;
        }
        let old = source(&tx, symbol, kind)?;
        let (id, role) = if let Some((id, z, role, _)) = old {
            if z != zone {
                return Err("Tick與K棒的時區標記不同，未自動轉換或混合補建".into());
            }
            if kind == "minute" && !["end", "start", "unspecified"].contains(&role.as_str()) {
                return Err("分鐘來源時間標記無效".into());
            }
            (id, role)
        } else {
            sql(tx.execute("INSERT INTO datasets(symbol,source,kind,timezone,timestamp_role,path) VALUES(?1,'tick_derived',?2,?3,?4,'')",params![symbol,kind,zone,if kind=="daily"{"end"}else{"end"}]))?;
            let id = tx.last_insert_rowid();
            sql(tx.execute(
                "INSERT INTO market_sources VALUES(?1,?2,?3)",
                params![symbol, kind, id],
            ))?;
            (id, "end".to_owned())
        };
        let mut ordinal: i64 = sql(tx.query_row(
            "SELECT COALESCE(MAX(ordinal),0) FROM records WHERE dataset_id=?1",
            [id],
            |r| r.get(0),
        ))?;
        let mut existing = BTreeSet::new();
        let mut stmt = sql(tx.prepare("SELECT datetime FROM records WHERE dataset_id=?1"))?;
        for value in sql(stmt.query_map([id], |r| r.get::<_, String>(0)))? {
            let t = value.map_err(|e| e.to_string())?;
            existing.insert(if kind == "daily" {
                t[..10].to_owned()
            } else {
                iso(parse_dt(&t)?)
            });
        }
        drop(stmt);
        let mut written = 0;
        for (key, a) in bars {
            let time = if kind == "minute" && role == "start" {
                iso(parse_dt(&key)? - Duration::minutes(1))
            } else {
                key.clone()
            };
            if existing.contains(&time) {
                continue;
            }
            ordinal += 1;
            let payload = a.payload().to_string();
            sql(tx.execute(
                "INSERT INTO records VALUES(?1,?2,?3,NULL,?4)",
                params![id, ordinal, time, payload],
            ))?;
            sql(tx.execute(
                "INSERT INTO market_bar_keys VALUES(?1,?2,?3)",
                params![id, time, ordinal],
            ))?;
            sql(tx.execute("INSERT OR REPLACE INTO qm_derived(dataset_id,time_key,tick_dataset_id,tick_sha256,profile) VALUES(?1,?2,?3,?4,?5)",params![id,time,tick_id,hash,profile]))?;
            written += 1;
        }
        sql(tx.execute("UPDATE datasets SET row_count=(SELECT COUNT(*) FROM records WHERE dataset_id=?1),first_datetime=(SELECT MIN(datetime) FROM records WHERE dataset_id=?1),last_datetime=(SELECT MAX(datetime) FROM records WHERE dataset_id=?1),sha256='mixed_tick_derived',imported_at=CURRENT_TIMESTAMP WHERE id=?1",[id]))?;
        added.insert(kind, written);
    }
    let receipt = json!({"symbol":symbol,"from":from,"to":to,"tick_rows_read":count,"inserted":added,"policy":"只填不存在的時間，保留所有既有棒；日K直接由Tick按設定交易日彙整","warning":"Tick可能本身缺漏，衍生K棒仍須核對；沒有Tick的分鐘不填零量或沿用價格"});
    sql(tx.execute(
        "INSERT INTO qm_events(symbol,action,details) VALUES(?1,'fill_from_ticks',?2)",
        params![symbol, receipt.to_string()],
    ))?;
    sql(tx.commit())?;
    Ok(receipt)
}
pub fn import(
    path: &Path,
    file: &Path,
    symbol: &str,
    kind: &str,
    zone: &str,
    role: &str,
) -> Result<Value, String> {
    let instrument = get(path, symbol)?;
    let receipt = canonical_market::import(path, file, symbol, kind, zone, role)?;
    // 原始匯入先單獨原子提交；自動補建失敗不冒稱匯入回滾。
    let auto = if kind == "tick" && instrument.auto_fill {
        let c = database::open_existing(path)?;
        let id = source(&c, symbol, "tick")?.unwrap().0;
        let (a, b): (String, String) = sql(c.query_row(
            "SELECT first_datetime,last_datetime FROM datasets WHERE id=?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        ))?;
        drop(c);
        let first = receipt["first"].as_str().unwrap_or(&a);
        let last = receipt["last"].as_str().unwrap_or(&b);
        let start = parse_day(&first[..10])?;
        let end = parse_day(&last[..10])?;
        let mut cursor = start;
        let mut results = vec![];
        let mut failure = None;
        while cursor <= end {
            // 交界多讀8日讓跨夜／週末交易日完整；只填缺少時間，因此可安全重疊。
            let finish = (cursor + Duration::days(90)).min(end);
            match fill_from_ticks(
                path,
                symbol,
                &(cursor - Duration::days(8)).to_string(),
                &finish.to_string(),
            ) {
                Ok(v) => results.push(v),
                Err(e) => {
                    failure = Some(e);
                    break;
                }
            }
            cursor = finish + Duration::days(1);
        }
        if let Some(error) = failure {
            json!({"status":"failed","error":error,"import_committed":true,"completed_chunks":results})
        } else {
            json!({"status":"completed","result":results})
        }
    } else {
        Value::Null
    };
    Ok(json!({"import":receipt,"auto_fill":auto}))
}
pub fn export(
    path: &Path,
    symbol: &str,
    kind: &str,
    from: &str,
    to: &str,
    target: &Path,
) -> Result<Value, String> {
    let (lo, hi) = range(from, to)?;
    let _ = get(path, symbol)?;
    if hi < lo {
        return Err("匯出起迄不可倒置".into());
    }
    let c = database::open_existing(path)?;
    let (id, _, _, _) = source(&c, symbol, kind)?.ok_or("沒有指定來源可匯出")?;
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(target)
        .map_err(|e| format!("無法新建匯出檔（不覆寫既有檔）：{e}"))?;
    let mut writer = csv::Writer::from_writer(file);
    let mut count = 0usize;
    let operation = (|| -> Result<(), String> {
        if kind == "tick" {
            writer
                .write_record(["Date", "Time", "Price", "Volume", "Nanosecond"])
                .map_err(|e| e.to_string())?;
            count = scan_ticks(&c, id, lo, hi, |t, p, v| {
                writer
                    .write_record([
                        t.format("%Y/%m/%d").to_string(),
                        t.format("%H:%M:%S").to_string(),
                        tick_bars::text(p as i128),
                        tick_bars::text(v as i128),
                        t.nanosecond().to_string(),
                    ])
                    .map_err(|e| e.to_string())
            })?;
        } else {
            writer
                .write_record([
                    "Date",
                    "Time",
                    "Open",
                    "High",
                    "Low",
                    "Close",
                    "TotalVolume",
                ])
                .map_err(|e| e.to_string())?;
            let mut stmt=sql(c.prepare("SELECT datetime,payload FROM records WHERE dataset_id=?1 AND datetime>=?2 AND datetime<=?3 ORDER BY datetime,ordinal"))?;
            let rows = sql(stmt.query_map(
                params![id, lo.date().to_string(), format!("{}~", hi.date())],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
            ))?;
            for row in rows {
                let (time, payload) = row.map_err(|e| e.to_string())?;
                let v: Value = serde_json::from_str(&payload).map_err(|e| e.to_string())?;
                let t = if time.len() == 10 {
                    parse_day(&time)?.and_hms_opt(0, 0, 0).unwrap()
                } else {
                    parse_dt(&time)?
                };
                writer
                    .write_record([
                        t.format("%Y/%m/%d").to_string(),
                        t.format("%H:%M:%S").to_string(),
                        v["open"].as_str().ok_or("非K棒來源")?.into(),
                        v["high"].as_str().unwrap_or("").into(),
                        v["low"].as_str().unwrap_or("").into(),
                        v["close"].as_str().unwrap_or("").into(),
                        v["volume"].as_str().unwrap_or("").into(),
                    ])
                    .map_err(|e| e.to_string())?;
                count += 1;
            }
        }
        writer.flush().map_err(|e| e.to_string())
    })();
    drop(writer);
    if let Err(e) = operation {
        let _ = fs::remove_file(target);
        return Err(e);
    }
    Ok(json!({"symbol":symbol,"kind":kind,"rows":count,"path":target,"from":from,"to":to}))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn backtest_coverage_snapshots_missing_stale_and_different_sources() {
        let (dir, db) = setup();
        let file = dir.join("backtest-coverage.csv");
        fs::write(&file, "Date,Time,Open,High,Low,Close,Volume\n2026/10/26,09:01:00,100,100,100,100,1\n2026/10/26,09:03:00,100,100,100,100,1\n").unwrap();
        import(&db, &file, "QM_TEST", "minute", "Asia/Taipei", "end").unwrap();
        let (id, timezone, role, hash) = source(&database::open_existing(&db).unwrap(), "QM_TEST", "minute").unwrap().unwrap();
        let mut report = json!({"first_bar":"2026-10-26T09:01:00","last_bar":"2026-10-26T09:03:00",
            "provenance":{"dataset_id":id,"symbol":"QM_TEST","kind":"minute","source":"test",
                "source_sha256":hash,"timezone":timezone,"source_timestamp_role":role}});
        assert_eq!(backtest_coverage(&db, &report)["status"], "not_checked");
        coverage(&db, "QM_TEST", "2026/10/26", "2026/10/26").unwrap();
        let verified = backtest_coverage(&db, &report);
        assert_eq!(verified["status"], "checked");
        assert_eq!(verified["last_check"]["covers_run"], true);
        assert_eq!(verified["overlapping_gap_examples"][0]["from"], "2026-10-26T09:02:00.000000000");
        report["last_bar"] = json!("2026-10-27T09:03:00");
        assert_eq!(backtest_coverage(&db, &report)["status"], "partial_range");
        let profile = get(&db, "QM_TEST").unwrap();
        save(&db, &profile).unwrap();
        let stale = backtest_coverage(&db, &report);
        assert_eq!(stale["status"], "stale");
        assert_eq!(stale["last_check"]["stale"], true);
        report["provenance"]["source_sha256"] = json!("older-version");
        let mismatch = backtest_coverage(&db, &report);
        assert_eq!(mismatch["status"], "source_mismatch");
        assert!(mismatch["overlapping_gap_examples"].is_null());
    }
    #[test]
    fn automatic_fill_uses_source_union_across_years_without_date_picker(){
        let (dir,db)=setup();
        let file=dir.join("automatic-fill.csv");fs::write(&file,"Date,Time,Price,Volume\n2020/01/01,09:00:30,100,1\n2022/01/04,09:00:30,101,2\n").unwrap();
        import(&db,&file,"QM_TEST","tick","Asia/Taipei","event").unwrap();
        let result=fill_all(&db,"QM_TEST").unwrap();
        assert_eq!(result["status"],"completed");assert_eq!(result["from"],"2020-01-01");assert_eq!(result["to"],"2022-01-04");assert_eq!(result["completed_chunks"],3);
        assert_eq!(result["inserted"]["minute"],2);assert_eq!(result["inserted"]["daily"],2);
        let repeated=fill_all(&db,"QM_TEST").unwrap();assert_eq!(repeated["inserted"]["minute"],0);assert_eq!(repeated["inserted"]["daily"],0);
    }
    #[test]
    fn widest_three_sources_checks_years_without_day_limit_and_chunk_results_match(){
        let (dir,db)=setup();
        let tick=dir.join("wide-tick.csv");fs::write(&tick,"Date,Time,Price,Volume\n2020/01/01,09:01:00,100,1\n").unwrap();
        import(&db,&tick,"QM_TEST","tick","Asia/Taipei","event").unwrap();
        let minute=dir.join("wide-minute.csv");fs::write(&minute,"Date,Time,Open,High,Low,Close,Volume\n2023/01/02,09:01:00,100,100,100,100,1\n").unwrap();
        import(&db,&minute,"QM_TEST","minute","Asia/Taipei","end").unwrap();
        let daily=dir.join("wide-day.csv");fs::write(&daily,"Date,Open,High,Low,Close,Volume\n2026/10/26,100,100,100,100,1\n").unwrap();
        import(&db,&daily,"QM_TEST","daily","Asia/Taipei","end").unwrap();
        let all=coverage_all(&db,"QM_TEST").unwrap();
        assert_eq!(all["from"],"2020-01-01");assert_eq!(all["to"],"2026-10-26");
        assert_eq!(all["sources"].as_array().unwrap().len(),3);assert!(all["sources"][1]["expected"].as_u64().unwrap()>1000);
        let grouped=coverage(&db,"QM_TEST","2026-10-19","2026-11-02").unwrap();
        let direct=compute_coverage(&db,"QM_TEST","2026-10-19","2026-11-02").unwrap();
        for k in 0..3{for key in ["expected","observed","missing","gap_count","tick_count","missing_seconds"]{assert_eq!(grouped["sources"][k][key],direct["sources"][k][key],"source {k} {key}");}}
        let mut night=get(&db,"QM_TEST").unwrap();night.sessions[0].open="23:00".into();night.sessions[0].close="01:00".into();save(&db,&night).unwrap();
        for text in ["2026/10/26,23:58:00,100,1\n2026/10/27,00:02:00,100,1\n","2026/10/26,23:57:00,100,1\n2026/10/27,00:00:00,100,1\n2026/10/27,00:03:00,100,1\n"]{
            fs::write(&tick,format!("Date,Time,Price,Volume\n{text}")).unwrap();import(&db,&tick,"QM_TEST","tick","Asia/Taipei","event").unwrap();
            let chunked=coverage(&db,"QM_TEST","2026-10-20","2026-10-28").unwrap();let whole=compute_coverage(&db,"QM_TEST","2026-10-20","2026-10-28").unwrap();
            for key in ["gap_count","tick_count","missing_seconds","gaps"]{assert_eq!(chunked["sources"][0][key],whole["sources"][0][key],"跨午夜 {key}");}
        }
        let empty=defaults("EMPTY_WIDE");save(&db,&empty).unwrap();assert!(coverage_all(&db,"EMPTY_WIDE").is_err());
    }
    #[test]
    fn last_check_persists_and_detects_import_and_settings_without_discarding_result(){
        let (dir,db)=setup();
        let first=coverage(&db,"QM_TEST","2026/10/26","2026/10/26").unwrap();
        assert_eq!(first["stale"],false);assert!(first["checked_at"].as_str().unwrap().contains('/'));
        let read=||{let c=database::open_existing(&db).unwrap();last_coverage(&c,"QM_TEST").unwrap()};
        initialize(&crate::workbench::root(),&db).unwrap();
        assert_eq!(read(),first); // 模擬重開，紀錄仍在資料庫。
        let file=dir.join("coverage-import.csv");
        fs::write(&file,"Date,Time,Open,High,Low,Close,Volume\n2026/10/26,09:01:00,100,100,100,100,1\n").unwrap();
        import(&db,&file,"QM_TEST","minute","Asia/Taipei","end").unwrap();
        let stale=read();assert_eq!(stale["stale"],true);assert_eq!(stale["sources"],first["sources"]);
        assert!(coverage(&db,"QM_TEST","bad-date","2026/10/26").is_err());
        assert_eq!(read(),stale); // 失敗不覆蓋上次成功紀錄。
        let fresh=coverage(&db,"QM_TEST","2026/10/26","2026/10/26").unwrap();assert_eq!(fresh["stale"],false);
        let mut other=defaults("OTHER");save(&db,&other).unwrap();other.name="另一商品".into();save(&db,&other).unwrap();assert_eq!(read()["stale"],false);
        let mut profile=get(&db,"QM_TEST").unwrap();profile.sessions[0].close="09:04".into();save(&db,&profile).unwrap();
        assert_eq!(read()["stale"],true);assert_eq!(read()["sources"],fresh["sources"]);
        assert_eq!(inventory(&db).unwrap().as_array().unwrap().iter().find(|r|r["instrument"]["symbol"]=="QM_TEST").unwrap()["last_coverage"]["stale"],true);
    }
    #[test]
    fn tick_timeline_threshold_is_three_minutes_and_never_bridges_sessions(){
        let dir=crate::workbench::root().join("outputs").join(format!("tick-timeline-test-{}",std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        fs::create_dir_all(&dir).unwrap();let db=dir.join("market.sqlite");initialize(&crate::workbench::root(),&db).unwrap();
        let mut i=defaults("TIME_TEST");i.english_name="Timeline Test".into();i.name="時間軸測試".into();
        i.sessions=vec![TradingSession{name:"早盤".into(),open:"09:00".into(),close:"09:15".into(),weekdays:vec![1,2,3,4,5],trading_day_shift:0},TradingSession{name:"午盤".into(),open:"10:00".into(),close:"10:05".into(),weekdays:vec![1,2,3,4,5],trading_day_shift:0}];save(&db,&i).unwrap();
        let file=dir.join("tick.csv");fs::write(&file,"Date,Time,Price,Volume\n2026/10/26,09:00:00,100,1\n2026/10/26,09:02:59.999999999,100,1\n2026/10/26,09:05:59.999999999,100,1\n2026/10/26,09:06:00,100,1\n2026/10/26,09:09:00.000000001,100,1\n2026/10/26,10:00:00,100,1\n2026/10/26,10:00:30,100,1\n").unwrap();
        import(&db,&file,"TIME_TEST","tick","Asia/Taipei","event").unwrap();
        let result=coverage(&db,"TIME_TEST","2026/10/26","2026/10/26").unwrap();let t=&result["sources"][0];
        assert_eq!(result["tick_gap_threshold_seconds"],180);assert_eq!(t["gap_count"],4);assert_eq!(t["missing"],4);
        assert_eq!(t["gaps"][0]["from"],"2026-10-26T09:02:59.999999999");
        assert_eq!(t["gaps"][0]["to"],"2026-10-26T09:05:59.999999999");
        assert!(t["timeline"].as_array().unwrap().iter().any(|s|s["status"]=="present"));
        let covered:i64=t["timeline"].as_array().unwrap().iter().map(|s|s["end_ms"].as_i64().unwrap()-s["start_ms"].as_i64().unwrap()).sum();
        assert_eq!(covered,20*60*1000); // 全交易時段皆分類，休盤的55分鐘不加入。
        let empty=defaults("NO_TICKS");save(&db,&empty).unwrap();let e=coverage(&db,"NO_TICKS","2026/10/26","2026/10/26").unwrap();
        assert_eq!(e["sources"][0]["missing"],1);assert_eq!(e["sources"][0]["timeline"][0]["status"],"missing");assert_eq!(e["sources"][0]["available"],false);
        assert_eq!(get(&db,"TIME_TEST").unwrap().english_name,"Timeline Test");
    }
    #[test]
    fn deletion_requires_fresh_plan_and_keeps_other_products_and_original_files(){
        let dir=crate::workbench::root().join("outputs").join(format!("delete-product-test-{}",std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        fs::create_dir_all(&dir).unwrap();let db=dir.join("market.sqlite");
        initialize(&crate::workbench::root(),&db).unwrap();
        for symbol in ["DELETE_TEST","KEEP_TEST","EMPTY_TEST"]{save(&db,&defaults(symbol)).unwrap();}
        let file=dir.join("minute.csv");fs::write(&file,"Date,Time,Open,High,Low,Close,Volume\n2026/10/26,09:01:00,100,101,99,100,1\n").unwrap();
        for symbol in ["DELETE_TEST","KEEP_TEST"] {import(&db,&file,symbol,"minute","Asia/Taipei","end").unwrap();}
        let plan=delete_plan(&db,"DELETE_TEST").unwrap();let token=plan["token"].as_str().unwrap();
        assert!(delete_instrument(&db,"DELETE_TEST","KEEP_TEST",token,true).is_err());
        assert!(delete_instrument(&db,"DELETE_TEST","DELETE_TEST",token,false).is_err());
        let ticks=dir.join("ticks.csv");fs::write(&ticks,"Date,Time,Price,Volume\n2026/10/26,09:00:00,100,1\n").unwrap();
        import(&db,&ticks,"DELETE_TEST","tick","Asia/Taipei","event").unwrap();
        assert!(delete_instrument(&db,"DELETE_TEST","DELETE_TEST",token,true).is_err());
        let fresh=delete_plan(&db,"DELETE_TEST").unwrap();
        delete_instrument(&db,"DELETE_TEST","DELETE_TEST",fresh["token"].as_str().unwrap(),true).unwrap();
        initialize(&crate::workbench::root(),&db).unwrap();
        assert!(get(&db,"DELETE_TEST").is_err());
        assert_eq!(delete_plan(&db,"KEEP_TEST").unwrap()["datasets"][0]["rows"],1);
        let c=database::open_existing(&db).unwrap();
        assert_eq!(c.query_row("SELECT COUNT(*) FROM datasets WHERE symbol='DELETE_TEST'",[],|r|r.get::<_,i64>(0)).unwrap(),0);
        assert_eq!(c.query_row("PRAGMA foreign_key_check",[],|_|Ok(())).optional().unwrap(),None);
        assert!(file.is_file()&&ticks.is_file());
        let empty=delete_plan(&db,"EMPTY_TEST").unwrap();
        delete_instrument(&db,"EMPTY_TEST","EMPTY_TEST",empty["token"].as_str().unwrap(),false).unwrap();
    }
    #[test]
    fn slash_dates_and_second_clock_export_tick_roundtrip_preserve_nanoseconds(){
        let dir=crate::workbench::root().join("outputs").join(format!("date-display-test-{}",std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        fs::create_dir_all(&dir).unwrap();let db=dir.join("market.sqlite");initialize(&crate::workbench::root(),&db).unwrap();
        let mut i=defaults("DATE_TEST");i.sessions[0].open="00:00:00".into();i.sessions[0].close="00:00:00".into();i.closed_dates=vec!["2026/10/27".into()];save(&db,&i).unwrap();
        assert_eq!(get(&db,"DATE_TEST").unwrap().closed_dates,vec!["2026-10-27"]);
        assert!(parse_time("05:00:01").is_err());
        let file=dir.join("tick.csv");fs::write(&file,"Date,Time,Price,Volume\n2026/10/26,09:00:00.000000001,100,1\n2026/10/26,09:00:00.123456789,101,2\n2026/10/26,09:00:00.123456789,101,2\n").unwrap();
        import(&db,&file,"DATE_TEST","tick","Asia/Taipei","event").unwrap();
        let out=dir.join("tick-export.csv");export(&db,"DATE_TEST","tick","2026/10/26","2026/10/26",&out).unwrap();
        let text=fs::read_to_string(&out).unwrap();assert!(text.contains("09:00:00,100,1,1"));assert!(!text.contains("09:00:00."));
        save(&db,&defaults("DATE_COPY")).unwrap();import(&db,&out,"DATE_COPY","tick","Asia/Taipei","event").unwrap();
        let c=database::open_existing(&db).unwrap();let a=canonical_market::resolve(&db,"DATE_TEST","tick").unwrap();let b=canonical_market::resolve(&db,"DATE_COPY","tick").unwrap();
        let rows=|id|crate::tick_store::query(&c,id,"","z",100).unwrap().iter().map(|r|r.datetime.clone()).collect::<Vec<_>>();
        assert_eq!(rows(a),rows(b));assert_eq!(rows(a).len(),3);
    }
    fn setup() -> (std::path::PathBuf, std::path::PathBuf) {
        let dir = crate::workbench::root().join("outputs").join(format!(
            "quote-manager-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        let db = dir.join("market.sqlite");
        initialize(&crate::workbench::root(), &db).unwrap();
        let mut i = defaults("QM_TEST");
        i.sessions = vec![TradingSession {
            name: "日盤".into(),
            open: "09:00".into(),
            close: "09:03".into(),
            weekdays: vec![1, 2, 3, 4, 5],
            trading_day_shift: 0,
        }];
        i.trading_weekdays = vec![1, 2, 3, 4, 5];
        save(&db, &i).unwrap();
        (dir, db)
    }
    #[test]
    fn sessions_validate_overlap_weekends_and_task_contract() {
        let (_, db) = setup();
        let mut i = get(&db, "QM_TEST").unwrap();
        i.sessions.push(i.sessions[0].clone());
        assert!(save(&db, &i).is_err());
        let v = coverage(&db, "QM_TEST", "2026-10-24", "2026-10-26").unwrap();
        assert_eq!(v["sources"][1]["expected"], 3);
        assert_eq!(v["sources"][1]["missing"], 3);
        let mut i = get(&db, "QM_TEST").unwrap();
        i.closed_dates.push("2026-10-26".into());
        save(&db, &i).unwrap();
        assert_eq!(
            coverage(&db, "QM_TEST", "2026-10-26", "2026-10-26").unwrap()["sources"][1]["expected"],
            0
        );
        let mut t = json!({"parameters":{},"market":{}});
        apply_to_task(&db, "QM_TEST", "minutes", &mut t).unwrap();
        assert_eq!(t["parameters"]["sessions"][0]["start_minute"], 540);
        assert_eq!(t["parameters"]["price_scale"], 1.0);
    }
    #[test]
    fn tick_fill_preserves_existing_duplicate_trades_and_close_boundary() {
        let (dir, db) = setup();
        let tick = dir.join("ticks.csv");
        fs::write(&tick,"Date,Time,Price,Volume\n2026/10/26,09:00:00,100,2\n2026/10/26,09:00:00,100,2\n2026/10/26,09:01:02,110,1\n2026/10/26,09:03:00,999,7\n").unwrap();
        import(&db, &tick, "QM_TEST", "tick", "Asia/Taipei", "event").unwrap();
        let minute = dir.join("minute.csv");
        fs::write(
            &minute,
            "Date,Time,Open,High,Low,Close,TotalVolume\n2026/10/26,09:01:00,90,90,90,90,9\n",
        )
        .unwrap();
        import(&db, &minute, "QM_TEST", "minute", "Asia/Taipei", "end").unwrap();
        let report = fill_from_ticks(&db, "QM_TEST", "2026-10-26", "2026-10-26").unwrap();
        assert_eq!(report["inserted"]["minute"], 1);
        assert_eq!(report["inserted"]["daily"], 1);
        let c = database::open_existing(&db).unwrap();
        let id = source(&c, "QM_TEST", "minute").unwrap().unwrap().0;
        let rows = database::query(&c, id, "", "z", 100).unwrap();
        assert_eq!(rows.len(), 2);
        assert!(matches!(&rows[0].payload,database::Payload::Bar{close,..} if close=="90"));
        let daily = source(&c, "QM_TEST", "daily").unwrap().unwrap().0;
        let rows = database::query(&c, daily, "", "z", 100).unwrap();
        assert!(
            matches!(&rows[0].payload,database::Payload::Bar{high,volume,..} if high=="110" && volume=="5")
        );
        drop(c);
        let cover = coverage(&db, "QM_TEST", "2026-10-26", "2026-10-26").unwrap();
        assert_eq!(cover["sources"][0]["observed"], 2);
        assert_eq!(cover["sources"][1]["missing"], 1);
        let again = fill_from_ticks(&db, "QM_TEST", "2026-10-26", "2026-10-26").unwrap();
        assert_eq!(again["inserted"]["minute"], 0);
        let out = dir.join("export.csv");
        assert_eq!(
            export(&db, "QM_TEST", "tick", "2026-10-26", "2026-10-26", &out).unwrap()["rows"],
            4
        );
        let before = fs::read(&out).unwrap();
        assert!(export(&db, "QM_TEST", "tick", "2026-10-26", "2026-10-26", &out).is_err());
        assert_eq!(before, fs::read(out).unwrap());
    }
    #[test]
    fn night_trading_date_weekend_and_clipped_range() {
        let (dir, db) = setup();
        let mut i = get(&db, "QM_TEST").unwrap();
        i.sessions = vec![TradingSession {
            name: "夜盤".into(),
            open: "23:00".into(),
            close: "01:00".into(),
            weekdays: vec![5],
            trading_day_shift: 1,
        }];
        save(&db, &i).unwrap();
        let windows = windows(
            &i,
            parse_day("2026-10-23").unwrap(),
            parse_day("2026-10-26").unwrap(),
        )
        .unwrap();
        assert_eq!(windows[0].day.to_string(), "2026-10-26");
        let tick = dir.join("night.csv");
        fs::write(
            &tick,
            "Date,Time,Price,Volume\n2026/10/23,23:00:00,100,1\n2026/10/24,00:10:00,110,1\n",
        )
        .unwrap();
        import(&db, &tick, "QM_TEST", "tick", "Asia/Taipei", "event").unwrap();
        assert_eq!(
            fill_from_ticks(&db, "QM_TEST", "2026-10-24", "2026-10-26").unwrap()["inserted"]
                ["daily"],
            0
        );
        assert_eq!(
            fill_from_ticks(&db, "QM_TEST", "2026-10-23", "2026-10-26").unwrap()["inserted"]
                ["daily"],
            1
        );
    }
    #[test]
    fn auto_fill_on_import_and_atomic_zone_mismatch() {
        let (dir, db) = setup();
        let mut i = get(&db, "QM_TEST").unwrap();
        i.auto_fill = true;
        save(&db, &i).unwrap();
        let tick = dir.join("auto.csv");
        fs::write(&tick, "Date,Time,Price,Volume\n2026/10/26,09:00:00,100,1\n").unwrap();
        let receipt = import(&db, &tick, "QM_TEST", "tick", "Asia/Taipei", "event").unwrap();
        assert_eq!(receipt["auto_fill"]["status"], "completed");
        let out = dir.join("daily.csv");
        export(&db, "QM_TEST", "daily", "2026-10-26", "2026-10-26", &out).unwrap();
        assert!(fs::read_to_string(out).unwrap().contains("100"));
        assert!(coverage(&db, "QM_TEST", "2024-01-01", "2026-01-01").is_ok());
    }
    #[test]
    fn annotation_and_provenance_replace_are_audited_without_changing_prices() {
        let (dir, db) = setup();
        let ticks = dir.join("ticks.csv");
        fs::write(
            &ticks,
            "Date,Time,Price,Volume\n2026/10/26,09:00:00,100,1\n",
        )
        .unwrap();
        import(&db, &ticks, "QM_TEST", "tick", "Asia/Taipei", "event").unwrap();
        let minute = dir.join("minute.csv");
        fs::write(
            &minute,
            "Date,Time,Open,High,Low,Close,TotalVolume\n2026/10/26,09:02:00,90,90,90,90,1\n",
        )
        .unwrap();
        import(
            &db,
            &minute,
            "QM_TEST",
            "minute",
            "unspecified",
            "unspecified",
        )
        .unwrap();
        assert!(fill_from_ticks(&db, "QM_TEST", "2026-10-26", "2026-10-26").is_err());
        assert_eq!(
            inventory(&db).unwrap()[0]["sources"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        annotate_source(&db, "QM_TEST", "minute", "Asia/Taipei", "end").unwrap();
        fill_from_ticks(&db, "QM_TEST", "2026-10-26", "2026-10-26").unwrap();
        let before = inventory(&db).unwrap();
        let minute_source = before[0]["sources"]
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["kind"] == "minute")
            .unwrap();
        assert_eq!(minute_source["tick_derived_rows"], 1);
        fs::write(
            &minute,
            "Date,Time,Open,High,Low,Close,TotalVolume\n2026/10/26,09:01:00,120,120,120,120,1\n",
        )
        .unwrap();
        import(&db, &minute, "QM_TEST", "minute", "Asia/Taipei", "end").unwrap();
        let after = inventory(&db).unwrap();
        let minute_source = after[0]["sources"]
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["kind"] == "minute")
            .unwrap();
        assert_eq!(minute_source["tick_derived_rows"], 0);
        assert!(annotate_source(&db, "QM_TEST", "minute", "Asia/Taipei", "start").is_err());
    }
    #[test]
    fn midnight_end_labels_read_previous_minute_and_full_calendar_day() {
        let (dir, db) = setup();
        let mut i = defaults("QM_TEST");
        i.auto_fill = false;
        save(&db, &i).unwrap();
        let ticks = dir.join("midnight.csv");
        fs::write(&ticks,"Date,Time,Price,Volume\n2026/10/25,23:59:30,99,1\n2026/10/26,00:01:00,100,1\n2026/10/26,23:59:59,110,1\n").unwrap();
        import(&db, &ticks, "QM_TEST", "tick", "Asia/Taipei", "event").unwrap();
        let coverage = coverage(&db, "QM_TEST", "2026-10-26", "2026-10-26").unwrap();
        assert_eq!(coverage["sources"][0]["observed"], 2);
        let fill = fill_from_ticks(&db, "QM_TEST", "2026-10-26", "2026-10-26").unwrap();
        assert_eq!(fill["inserted"]["minute"], 2);
        assert_eq!(fill["inserted"]["daily"], 1);
        let c = database::open_existing(&db).unwrap();
        let id = source(&c, "QM_TEST", "daily").unwrap().unwrap().0;
        let rows = database::query(&c, id, "", "z", 10).unwrap();
        assert!(
            matches!(&rows[0].payload,database::Payload::Bar{open,close,volume,..}if open=="100"&&close=="110"&&volume=="2")
        );
    }
}
