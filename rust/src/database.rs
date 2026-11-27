//! Typed, transactional market-data storage. No model or broker dependency.
use chrono::{NaiveDate, NaiveDateTime, Timelike};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{collections::{HashMap, HashSet}, fs::File, io::{self, Read}, path::Path};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DataKind { Trade, Quote, Minute, Daily, Custom }
impl DataKind {
    pub fn parse(s: &str) -> Result<Self, String> {
        match s { "trade"=>Ok(Self::Trade), "quote"=>Ok(Self::Quote), "minute"=>Ok(Self::Minute), "daily"=>Ok(Self::Daily), "custom"=>Ok(Self::Custom), _=>Err("資料類型須為 trade、quote、minute、daily 或 custom".into()) }
    }
    pub fn name(self) -> &'static str { match self { Self::Trade=>"trade", Self::Quote=>"quote", Self::Minute=>"minute", Self::Daily=>"daily", Self::Custom=>"custom" } }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Level { pub price: String, pub volume: String }
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag="type", rename_all="snake_case")]
pub enum Payload {
    Trade { price: String, volume: String },
    Quote { price: String, volume: String, bids: Vec<Level>, asks: Vec<Level> },
    Bar { open: String, high: String, low: String, close: String, volume: String },
    Custom { value: String },
}
#[derive(Debug, Serialize, Deserialize)]
pub struct Record { pub datetime: String, pub available_at: Option<String>, pub payload: Payload }
#[derive(Debug, Serialize, Deserialize)]
pub struct ImportSummary { pub dataset_id: i64, pub kind: String, pub count: i64, pub first: String, pub last: String, pub sha256: String, pub timezone: String }

pub fn timestamp(value: &str, date_only: bool) -> Result<String, String> {
    let v=value.trim();
    if date_only {
        for fmt in ["%Y-%m-%d", "%Y/%m/%d", "%Y%m%d"] {
            if let Ok(d)=NaiveDate::parse_from_str(v,fmt) { return Ok(d.to_string()); }
        }
        // A daily timestamp may name the close time. Preserve it rather than truncate.
    }
    for fmt in ["%Y-%m-%d %H:%M:%S%.f", "%Y/%m/%d %H:%M:%S%.f", "%Y-%m-%dT%H:%M:%S%.f", "%Y%m%d %H:%M", "%Y%m%d %H:%M:%S", "%Y-%m-%d %H:%M", "%Y/%m/%d %H:%M"] {
        if let Ok(d)=NaiveDateTime::parse_from_str(v,fmt) {
            if d.second()>59 { return Err("不支援閏秒時間戳".into()); }
            return Ok(d.format("%Y-%m-%dT%H:%M:%S%.9f").to_string());
        }
    }
    Err(format!("無效日期時間或未支援的時區偏移：{value}"))
}

pub(crate) fn normalized(s: &str) -> String {
    match s.trim().trim_start_matches('\u{feff}').to_ascii_lowercase().as_str() {
        "日期"=>"date".into(),"日期時間"=>"datetime".into(),"時間"=>"time".into(),
        "開盤"|"開盤價"|"openpx"=>"open".into(),"最高"|"最高價"|"highpx"=>"high".into(),"最低"|"最低價"|"lowpx"=>"low".into(),"收盤"|"收盤價"|"closepx"=>"close".into(),"成交量"|"totalvolume"=>"volume".into(),"成交價"=>"price".into(),other=>other.into()
    }
}
pub(crate) struct Columns { map: HashMap<String, usize> }
impl Columns {
    pub(crate) fn new(headers: &csv::StringRecord, kind: DataKind) -> Result<Self,String> {
        let mut map=HashMap::new();
        for (i,h) in headers.iter().enumerate() {
            let key=normalized(h);
            if map.insert(key.clone(),i).is_some() { return Err(format!("欄位重複：{key}")); }
        }
        if !map.contains_key("datetime") && !map.contains_key("time") {if let Some(i)=map.get("date").copied(){map.insert("datetime".into(),i);}}
        let c=Self{map};
        if !c.map.contains_key("datetime") && (!c.map.contains_key("date") || (!c.map.contains_key("time") && ![DataKind::Daily,DataKind::Custom].contains(&kind))) { return Err("需有 datetime 或 Date＋Time；日 K 與自訂資料可僅有 Date".into()); }
        let fields: &[&str]=match kind { DataKind::Trade|DataKind::Quote=>&["price","volume"],DataKind::Minute|DataKind::Daily=>&["open","high","low","close","volume"],DataKind::Custom=>&["value"] };
        let missing=fields.iter().filter(|f|!c.map.contains_key(**f)).copied().collect::<Vec<_>>();
        if !missing.is_empty(){return Err(format!("缺少欄位：{}{}",missing.join("、"),if matches!(kind,DataKind::Minute|DataKind::Daily){"。完整K棒需要開盤價、最高價、最低價、收盤價、成交量；不能只用收盤價補造OHLC。"}else{""}));}
        if kind==DataKind::Quote {
            for side in ["bid","ask"] { for n in 1..=10 { for field in ["price","volume"] {
                let name=format!("{side}{n}_{field}");
                if !c.map.contains_key(&name) { return Err(format!("十檔報價缺少欄位：{name}；無報價檔位請保留空白")); }
            } } }
        }
        Ok(c)
    }
    fn get<'a>(&self,row:&'a csv::StringRecord,key:&str)->Result<&'a str,String> {
        self.map.get(key).and_then(|i|row.get(*i)).ok_or_else(||format!("缺少欄位：{key}"))
    }
    fn number(&self,row:&csv::StringRecord,key:&str,nonnegative:bool)->Result<String,String> {
        let v=self.get(row,key)?;
        let n=v.parse::<f64>().map_err(|_|format!("{key} 不是數值：{v}"))?;
        if !n.is_finite() || (nonnegative && n<0.0) { return Err(format!("{key} 數值不合理：{v}")); }
        // Preserve the original decimal spelling; database prices never round-trip via f64.
        Ok(v.into())
    }
    fn levels(&self,row:&csv::StringRecord,side:&str)->Result<Vec<Level>,String> {
        let mut out:Vec<Level>=Vec::new(); let mut gap=false;
        for n in 1..=10 {
            let p=format!("{side}{n}_price");let v=format!("{side}{n}_volume");
            let a=self.get(row,&p)?;let b=self.get(row,&v)?;
            if a.is_empty() && b.is_empty() { gap=true;continue; }
            if gap || a.is_empty() || b.is_empty() { return Err(format!("{side} 檔位須從一檔連續，價格與數量須成對")); }
            let level=Level{price:self.number(row,&p,false)?,volume:self.number(row,&v,true)?};
            if let Some(prev)=out.last() {
                let x=prev.price.parse::<f64>().unwrap();let y=level.price.parse::<f64>().unwrap();
                if (side=="bid" && y>=x)||(side=="ask" && y<=x) { return Err(format!("{side} 檔位價格順序錯誤")); }
            }
            out.push(level);
        }
        Ok(out)
    }
    pub(crate) fn record(&self,row:&csv::StringRecord,kind:DataKind)->Result<Record,String> {
        let raw=if self.map.contains_key("datetime") { self.get(row,"datetime")?.to_string() }
            else if self.map.contains_key("time") { format!("{} {}",self.get(row,"date")?,self.get(row,"time")?) }
            else { self.get(row,"date")?.to_string() };
        let datetime=timestamp(&raw,[DataKind::Daily,DataKind::Custom].contains(&kind))?;
        if kind==DataKind::Minute {
            let d=NaiveDateTime::parse_from_str(&datetime,"%Y-%m-%dT%H:%M:%S%.f").unwrap();
            if d.second()!=0 || d.nanosecond()!=0 { return Err("1 分 K 須對齊整分鐘".into()); }
        }
        let available_at=if self.map.contains_key("available_at") && !self.get(row,"available_at")?.is_empty() {
            Some(timestamp(self.get(row,"available_at")?,false)?)
        } else { None };
        let payload=match kind {
            DataKind::Trade=>Payload::Trade{price:self.number(row,"price",false)?,volume:self.number(row,"volume",true)?},
            DataKind::Quote=>Payload::Quote{price:self.number(row,"price",false)?,volume:self.number(row,"volume",true)?,bids:self.levels(row,"bid")?,asks:self.levels(row,"ask")?},
            DataKind::Custom=>Payload::Custom{value:self.number(row,"value",false)?},
            DataKind::Minute|DataKind::Daily=>{
                let open=self.number(row,"open",false)?;let high=self.number(row,"high",false)?;let low=self.number(row,"low",false)?;let close=self.number(row,"close",false)?;let volume=self.number(row,"volume",true)?;
                let o=open.parse::<f64>().unwrap();let h=high.parse::<f64>().unwrap();let l=low.parse::<f64>().unwrap();let c=close.parse::<f64>().unwrap();
                if h<l || h<o.max(c) || l>o.min(c) { return Err("OHLC 最高／最低價不合理".into()); }
                Payload::Bar{open,high,low,close,volume}
            }
        };
        Ok(Record{datetime,available_at,payload})
    }
}

const SCHEMA:&str="
CREATE TABLE datasets(
 id INTEGER PRIMARY KEY, symbol TEXT NOT NULL, source TEXT NOT NULL, kind TEXT NOT NULL CHECK(kind IN ('trade','quote','minute','daily','custom')),
 timezone TEXT NOT NULL, timestamp_role TEXT NOT NULL CHECK(timestamp_role IN ('start','end','event','unspecified')),
 path TEXT NOT NULL, sha256 TEXT, row_count INTEGER NOT NULL DEFAULT 0,
 first_datetime TEXT, last_datetime TEXT, imported_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
 UNIQUE(symbol,source,kind,timezone,timestamp_role,sha256)
) STRICT;
CREATE TABLE records(
 dataset_id INTEGER NOT NULL REFERENCES datasets(id), ordinal INTEGER NOT NULL,
 datetime TEXT NOT NULL, available_at TEXT, payload TEXT NOT NULL CHECK(json_valid(payload)),
 PRIMARY KEY(dataset_id,ordinal)
) STRICT;
CREATE INDEX records_time ON records(dataset_id,datetime,ordinal);
PRAGMA user_version=1;
PRAGMA application_id=1380270403;
";

pub fn open(path:&Path)->Result<Connection,String> {
    let conn=Connection::open(path).map_err(|e|e.to_string())?;
    conn.busy_timeout(std::time::Duration::from_secs(5)).map_err(|e|e.to_string())?;
    let version:i64=conn.pragma_query_value(None,"user_version",|r|r.get(0)).map_err(|e|e.to_string())?;
    let app:i64=conn.pragma_query_value(None,"application_id",|r|r.get(0)).map_err(|e|e.to_string())?;
    if version==0 && app==0 {
        let count:i64=conn.query_row("SELECT count(*) FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%'",[],|r|r.get(0)).map_err(|e|e.to_string())?;
        if count!=0 { return Err("資料庫已含未知資料表，拒絕初始化".into()); }
        conn.execute_batch(&format!("BEGIN IMMEDIATE;{SCHEMA}COMMIT;")).map_err(|e|e.to_string())?;
    } else if version!=1 || app!=1380270403 { return Err("資料庫版本或應用程式識別不相容".into()); }
    conn.execute_batch("PRAGMA foreign_keys=ON; PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;").map_err(|e|e.to_string())?;
    Ok(conn)
}

pub fn open_existing(path:&Path)->Result<Connection,String> {
    let conn=Connection::open_with_flags(path,rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).map_err(|e|e.to_string())?;
    conn.busy_timeout(std::time::Duration::from_secs(5)).map_err(|e|e.to_string())?;
    let version:i64=conn.pragma_query_value(None,"user_version",|r|r.get(0)).map_err(|e|e.to_string())?;
    let app:i64=conn.pragma_query_value(None,"application_id",|r|r.get(0)).map_err(|e|e.to_string())?;
    if version!=1 || app!=1380270403 { return Err("不是相容的行情資料庫".into()); }
    Ok(conn)
}

struct Hashed<R> { reader:R, hash:Sha256 }
impl<R:Read> Read for Hashed<R> {
    fn read(&mut self,b:&mut [u8])->io::Result<usize> { let n=self.reader.read(b)?;self.hash.update(&b[..n]);Ok(n) }
}

pub fn import_file(conn:&mut Connection,path:&Path,kind:DataKind,symbol:&str,source:&str,timezone:&str,role:&str)->Result<ImportSummary,String> {
    if [symbol,source,timezone].iter().any(|s|s.trim().is_empty()) { return Err("商品、來源及時區標記不得為空；未知時區填 unspecified".into()); }
    if !["start","end","event","unspecified"].contains(&role) { return Err("時間標記須為 start、end、event 或 unspecified".into()); }
    if [DataKind::Trade,DataKind::Quote,DataKind::Custom].contains(&kind) && ["start","end"].contains(&role) { return Err("事件資料不可標記成 K 棒起訖時間".into()); }
    if kind==DataKind::Trade {
        let r=crate::tick_store::import_into(conn,path,symbol,"normalized_csv",timezone)?;
        return Ok(ImportSummary{dataset_id:r.dataset_id,kind:"trade".into(),count:r.count as i64,first:r.first,last:r.last,sha256:r.source_sha256,timezone:r.timezone});
    }
    let file=File::open(path).map_err(|e|e.to_string())?;
    // Detect delimiter from a separate small read. The digest covers the import stream itself.
    let delimiter=crate::market_csv::delimiter(path)?;
    let mut reader=crate::market_csv::TextCsv::new(Hashed{reader:file,hash:Sha256::new()},delimiter)?;
    let columns=Columns::new(reader.headers().map_err(|e|e.to_string())?,kind)?;
    let tx=conn.transaction().map_err(|e|e.to_string())?;
    tx.execute("INSERT INTO datasets(symbol,source,kind,timezone,timestamp_role,path) VALUES(?1,?2,?3,?4,?5,?6)",params![symbol,source,kind.name(),timezone,role,path.to_string_lossy()]).map_err(|e|e.to_string())?;
    let id=tx.last_insert_rowid();let mut count=0i64;let mut start=String::new();let mut end=String::new();let mut times=HashSet::new();
    {
        let mut stmt=tx.prepare("INSERT INTO records(dataset_id,ordinal,datetime,available_at,payload) VALUES(?1,?2,?3,?4,?5)").map_err(|e|e.to_string())?;
        for (i,row) in reader.records().enumerate() {
            let parsed=row.map_err(|e|format!("第 {} 列：{e}",i+2))?;
            let record=columns.record(&parsed,kind).map_err(|e|format!("第 {} 列：{e}",i+2))?;
            let time_key=if kind==DataKind::Daily {record.datetime[..10].to_string()}else{record.datetime.clone()};
            if [DataKind::Minute,DataKind::Daily].contains(&kind) && !times.insert(time_key) { return Err(format!("第 {} 列 K 棒時間或日期重複：{}",i+2,record.datetime)); }
            if count==0 || record.datetime<start { start=record.datetime.clone(); }
            if count==0 || record.datetime>end { end=record.datetime.clone(); }
            count+=1;
            stmt.execute(params![id,count,record.datetime,record.available_at,serde_json::to_string(&record.payload).map_err(|e|e.to_string())?]).map_err(|e|e.to_string())?;
        }
    }
    if count==0 { return Err("資料檔沒有紀錄".into()); }
    let sha256=format!("{:x}",reader.into_inner().hash.finalize());
    tx.execute("UPDATE datasets SET sha256=?1,row_count=?2,first_datetime=?3,last_datetime=?4 WHERE id=?5",params![sha256,count,start,end,id]).map_err(|e|format!("資料集已存在或更新失敗，整批取消：{e}"))?;
    tx.commit().map_err(|e|e.to_string())?;
    Ok(ImportSummary{dataset_id:id,kind:kind.name().into(),count,first:start,last:end,sha256,timezone:timezone.into()})
}

pub fn query(conn:&Connection,id:i64,from:&str,to:&str,limit:usize)->Result<Vec<Record>,String> {
    if limit==0 || limit>100_000 { return Err("查詢筆數須為 1 至 100000".into()); }
    if crate::tick_store::contains(conn,id)? { return crate::tick_store::query(conn,id,from,to,limit); }
    let mut stmt=conn.prepare("SELECT datetime,available_at,payload FROM records WHERE dataset_id=?1 AND datetime>=?2 AND datetime<=?3 ORDER BY datetime,ordinal LIMIT ?4").map_err(|e|e.to_string())?;
    let rows=stmt.query_map(params![id,from,to,limit as i64],|r|Ok((r.get::<_,String>(0)?,r.get::<_,Option<String>>(1)?,r.get::<_,String>(2)?))).map_err(|e|e.to_string())?;
    rows.map(|r|{let (datetime,available_at,payload)=r.map_err(|e|e.to_string())?;Ok(Record{datetime,available_at,payload:serde_json::from_str(&payload).map_err(|e|e.to_string())?})}).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    fn parse(text:&str,kind:DataKind)->Result<Record,String> {
        let mut reader=csv::ReaderBuilder::new().trim(csv::Trim::All).from_reader(text.as_bytes());
        let c=Columns::new(reader.headers().unwrap(),kind)?;
        c.record(&reader.records().next().unwrap().unwrap(),kind)
    }
    #[test]fn five_kinds_and_decimal_preservation() {
        for (kind,text) in [(DataKind::Trade,"datetime,price,Volume\n2026-10-23 09:00:00.123456789,123.4500,2"),(DataKind::Minute,"Date,Time,Open,High,Low,Close,TotalVolume\n2026/10/23,09:00:00,10,12,9,11,100"),(DataKind::Daily,"datetime,OpenPx,highPx,lowPx,closePx,Volume\n2026-10-23,10,12,9,11,100"),(DataKind::Custom,"datetime,Value\n2026-10-23 09:00:00,-1.2500")] { assert!(parse(text,kind).is_ok()); }
        assert!(matches!(parse("datetime,price,volume\n2026-10-23 09:00:00,123.4500,2",DataKind::Trade).unwrap().payload,Payload::Trade{price,..} if price=="123.4500"));
        let mut h=vec!["datetime".to_string(),"price".into(),"volume".into()];let mut v=vec!["2026-10-23 09:00:00".to_string(),"100".into(),"1".into()];
        for side in ["bid","ask"] {for n in 1..=10 {h.extend([format!("{side}{n}_price"),format!("{side}{n}_volume")]);v.extend([(if side=="bid" {100-n}else{100+n}).to_string(),"1".into()]);}}
        let record=parse(&format!("{}\n{}",h.join(","),v.join(",")),DataKind::Quote).unwrap();
        assert!(matches!(record.payload,Payload::Quote{bids,asks,..} if bids.len()==10 && asks.len()==10));
    }
    #[test]fn rejects_bad_values_and_time() {
        assert!(parse("datetime,price,volume\n2026-10-23 09:00:00,NaN,1",DataKind::Trade).is_err());
        assert!(parse("datetime,OpenPx,highPx,lowPx,closePx,Volume\n2026-10-23 09:00:01,10,12,9,11,100",DataKind::Minute).is_err());
        assert!(parse("datetime,value\n2026-02-30 09:00:00,1",DataKind::Custom).is_err());
        assert!(parse("datetime,open,high,low,close,volume\n2026-10-23,10,8,9,11,100",DataKind::Daily).is_err());
        assert!(parse("datetime,price,volume,Volume\n2026-10-23 09:00:00,10,1,1",DataKind::Trade).is_err());
        assert_eq!(parse("datetime,open,high,low,close,volume\n2026-10-23 13:45:00,10,12,9,11,100",DataKind::Daily).unwrap().datetime,"2026-10-23T13:45:00.000000000");
    }
    #[test]fn transactions_duplicates_and_reopen() {
        let path=std::env::temp_dir().join(format!("recharts-db-{}.sqlite",std::process::id()));
        let input=path.with_extension("csv");
        {let mut c=open(&path).unwrap();
        std::fs::write(&input,"datetime,price,volume\n2026-10-23 09:00:00,1,2\n2026-10-23 09:00:00,1,2\n").unwrap();
        let summary=import_file(&mut c,&input,DataKind::Trade,"TXF1","test","unspecified","event").unwrap();
        assert_eq!(query(&c,summary.dataset_id,"","z",10).unwrap().len(),2);
        assert_eq!(import_file(&mut c,&input,DataKind::Trade,"TXF1","test","unspecified","event").unwrap().count,2);
        std::fs::write(&input,"datetime,price,volume\n2026-10-23 09:00:00,1,2\n2026-10-23 09:00:01,1,-1\n").unwrap();
        assert!(import_file(&mut c,&input,DataKind::Trade,"TXF1","test","unspecified","event").is_err());
        assert_eq!(c.query_row("SELECT count(*) FROM datasets",[],|r|r.get::<_,i64>(0)).unwrap(),1);}
        {let c=open(&path).unwrap();assert_eq!(query(&c,1,"","z",10).unwrap().len(),2);}
        std::fs::remove_file(path).unwrap();std::fs::remove_file(input).unwrap();
    }
}
