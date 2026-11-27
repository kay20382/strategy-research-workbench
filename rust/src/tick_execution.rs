//! Bounded-memory, ordered canonical Tick reader for bar-close strategies.
use crate::{backtest::{Candle, Config}, database, tick_bars::Aggregator, tick_store};
use chrono::NaiveDateTime;
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::path::Path;

#[derive(Clone, Debug)]
pub struct ExecutionTick {
    pub ns: i64,
    pub price: f64,
    pub(crate) volume: i64,
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> DatabaseTicks {
        let connection=Connection::open_in_memory().unwrap();
        connection.execute_batch("CREATE TABLE tick_blocks(dataset_id INTEGER, block_no INTEGER,start_ns INTEGER,end_ns INTEGER,tick_count INTEGER,data BLOB,sha256 TEXT);").unwrap();
        let times=["2026-10-23T08:45:00","2026-10-23T08:59:59","2026-10-23T09:00:00","2026-10-23T09:00:00","2026-10-23T13:45:00"];
        let prices=[100i64,105,110,108,115];
        let mut data=vec![];
        for (time,price) in times.iter().zip(prices) {
            let ns=NaiveDateTime::parse_from_str(time,"%Y-%m-%dT%H:%M:%S").unwrap().and_utc().timestamp_nanos_opt().unwrap();
            data.extend(ns.to_le_bytes());data.extend((price*1_000_000).to_le_bytes());data.extend(1_000_000i64.to_le_bytes());
        }
        let start=i64::from_le_bytes(data[..8].try_into().unwrap());
        let end=i64::from_le_bytes(data[data.len()-24..data.len()-16].try_into().unwrap());
        connection.execute("INSERT INTO tick_blocks VALUES(4,0,?,?,5,?,?)",params![start,end,data,format!("{:x}",Sha256::digest(&data))]).unwrap();
        DatabaseTicks {connection,id:4,block_no:-1,data:vec![],offset:0,previous_ns:None,peek:None,done:false,
            windows:Aggregator::with_sessions(900).unwrap(),hashes:Sha256::new(),
            proof:TickExecutionProof {dataset_id:4,symbol:"TXF1".into(),source_sha256:"test".into(),verified_blocks:0,verified_bars:0,consumed_ticks:0,skipped_before_first_bar:0,verified_block_hashes_sha256:String::new(),ordering:String::new(),limit_fill_policy:String::new()}}
    }
    fn bar(time:&str,o:f64,h:f64,l:f64,c:f64,v:f64)->Candle {
        Candle {datetime:format!("2026-10-23T{time}"),open:o,high:h,low:l,close:c,volume:v,pl_date:1261023,pl_time:900,source_rows:1}
    }
    #[test]
    fn reader_separates_boundary_and_keeps_duplicate_timestamp_records() {
        let mut f=fixture();
        let first=f.bar(&bar("09:00:00",100.,105.,100.,105.,2.)).unwrap();
        let next=f.bar(&bar("09:15:00",110.,110.,108.,108.,2.)).unwrap();
        assert_eq!(first.len(),2);assert_eq!(next.len(),2);
        assert_eq!(next[0].ns,next[1].ns);assert_eq!(next[0].price,110.);assert_eq!(next[1].price,108.);
        assert_eq!(f.proof().consumed_ticks,4);
        // The session-closing print belongs to the final session bar.
        assert_eq!(f.bar(&bar("13:45:00",115.,115.,115.,115.,1.)).unwrap().len(),1);
    }
    #[test]
    fn reader_rejects_missing_ticks_and_mismatching_ohlcv() {
        let mut f=fixture();assert!(f.bar(&bar("09:00:00",100.,106.,100.,105.,2.)).unwrap_err().contains("OHLCV"));
        let mut f=fixture();f.bar(&bar("09:00:00",100.,105.,100.,105.,2.)).unwrap();
        f.bar(&bar("09:15:00",110.,110.,108.,108.,2.)).unwrap();
        assert!(f.bar(&bar("09:30:00",108.,108.,108.,108.,0.)).unwrap_err().contains("無對應Tick"));
    }
    #[test]
    fn reader_rejects_tampered_blocks() {
        let mut f=fixture();f.connection.execute("UPDATE tick_blocks SET sha256='bad'",[]).unwrap();
        assert!(f.bar(&bar("09:00:00",100.,105.,100.,105.,2.)).unwrap_err().contains("雜湊"));
    }
}
impl ExecutionTick {
    pub fn datetime(&self) -> String {
        chrono::DateTime::from_timestamp(self.ns.div_euclid(1_000_000_000),
            self.ns.rem_euclid(1_000_000_000) as u32).unwrap()
            .format("%Y-%m-%dT%H:%M:%S%.9f").to_string()
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct TickExecutionProof {
    pub dataset_id: i64,
    pub symbol: String,
    pub source_sha256: String,
    pub verified_blocks: u64,
    pub verified_bars: u64,
    pub consumed_ticks: u64,
    pub skipped_before_first_bar: u64,
    pub verified_block_hashes_sha256: String,
    pub ordering: String,
    pub limit_fill_policy: String,
}

pub struct DatabaseTicks {
    connection: Connection,
    id: i64,
    block_no: i64,
    data: Vec<u8>,
    offset: usize,
    previous_ns: Option<i64>,
    peek: Option<ExecutionTick>,
    done: bool,
    windows: Aggregator,
    hashes: Sha256,
    proof: TickExecutionProof,
}
impl DatabaseTicks {
    pub fn open(path: &Path, cfg: &Config, symbol: &str, timezone: &str) -> Result<Self, String> {
        let id = cfg.tick_dataset_id.ok_or("Tick模式需要tick_dataset_id")?;
        if cfg.timestamp_role != "end" || (cfg.interval_minutes == 0 && cfg.interval_seconds.is_none()) {
            return Err("Tick撮合目前需要end時間標記及明確K棒週期".into());
        }
        let seconds=cfg.interval_seconds.map(i64::from).unwrap_or(cfg.interval_minutes as i64*60);
        let windows = if cfg.sessions.is_empty() {
            Aggregator::new(seconds, cfg.bar_anchor_minutes as i64 * 60)?
        } else {
            let mut sessions: Vec<_> = cfg.sessions.iter().map(|s| (s.start_minute,s.end_minute)).collect();
            sessions.sort();
            if sessions != vec![(525,825),(900,300)] {
                return Err("Tick撮合時段目前支援臺指日盤08:45及夜盤15:00；其他時段不可套用".into());
            }
            if cfg.interval_seconds.is_some(){Aggregator::with_sessions_close_exclusive(seconds)?}else{Aggregator::with_sessions(seconds)?}
        };
        let connection = database::open_existing(path)?;
        connection.execute_batch("BEGIN DEFERRED TRANSACTION").map_err(|e|e.to_string())?;
        if !tick_store::contains(&connection,id)? { return Err("Tick資料集不是標準tick_blocks格式".into()); }
        let (s,z,sha,kind):(String,String,String,String) = connection.query_row(
            "SELECT symbol,timezone,sha256,kind FROM datasets WHERE id=?",[id],
            |r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).map_err(|e|e.to_string())?;
        let canonical: bool = connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM tick_canonical WHERE dataset_id=? AND symbol=?)",
            params![id,symbol],|r|r.get(0)).map_err(|e|e.to_string())?;
        if s != symbol || z != timezone || kind != "trade" || !canonical {
            return Err("Tick商品、時區或canonical來源與K棒不符".into());
        }
        Ok(Self { connection,id,block_no:-1,data:vec![],offset:0,previous_ns:None,peek:None,done:false,
            windows,hashes:Sha256::new(),proof:TickExecutionProof {dataset_id:id,symbol:s,source_sha256:sha,
            verified_blocks:0,verified_bars:0,consumed_ticks:0,skipped_before_first_bar:0,
            verified_block_hashes_sha256:String::new(),ordering:"時間、原始區塊序、區塊內序；同時戳不重排、不去重".into(),
            limit_fill_policy:"成交價觸價即全成，採委託限價；不模擬排隊與部分成交".into() } })
    }
    fn read(&mut self) -> Result<Option<ExecutionTick>,String> {
        if self.offset == self.data.len() {
            if self.done { return Ok(None); }
            let block: Option<(i64,i64,i64,i64,Vec<u8>,String)> = self.connection.query_row(
                "SELECT block_no,start_ns,end_ns,tick_count,data,sha256 FROM tick_blocks WHERE dataset_id=? AND block_no>? ORDER BY block_no LIMIT 1",
                params![self.id,self.block_no],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?)))
                .optional().map_err(|e|e.to_string())?;
            let Some((no,start,end,count,data,hash))=block else {self.done=true;return Ok(None)};
            let number=|b:&[u8]| i64::from_le_bytes(b.try_into().unwrap());
            if no != self.block_no+1 || count<=0 || data.len()!=count as usize*24
                || format!("{:x}",Sha256::digest(&data)) != hash
                || number(&data[..8]) != start || number(&data[data.len()-24..data.len()-16]) != end {
                return Err("Tick區塊序、長度、起訖或雜湊不符".into());
            }
            self.hashes.update(no.to_le_bytes()); self.hashes.update(hash.as_bytes());
            self.proof.verified_blocks+=1;self.block_no=no;self.data=data;self.offset=0;
        }
        let row=&self.data[self.offset..self.offset+24];self.offset+=24;
        let ns=i64::from_le_bytes(row[..8].try_into().unwrap());
        let price=i64::from_le_bytes(row[8..16].try_into().unwrap());
        let volume=i64::from_le_bytes(row[16..24].try_into().unwrap());
        if self.previous_ns.is_some_and(|p|ns<p) || volume<0 {
            return Err("Tick時間倒退或成交量無效".into());
        }
        self.previous_ns=Some(ns);
        Ok(Some(ExecutionTick {ns,price:price as f64/1_000_000.0,volume}))
    }
    pub fn bar(&mut self, b:&Candle) -> Result<Vec<ExecutionTick>,String> {
        let end=NaiveDateTime::parse_from_str(&b.datetime,"%Y-%m-%dT%H:%M:%S%.f")
            .map_err(|e|e.to_string())?.and_utc().timestamp();
        let mut out=vec![];
        loop {
            if self.peek.is_none() {self.peek=self.read()?;}
            let Some(t)=self.peek.as_ref() else {break};
            let dt=chrono::DateTime::from_timestamp(t.ns.div_euclid(1_000_000_000),t.ns.rem_euclid(1_000_000_000) as u32)
                .ok_or("Tick時間超出範圍")?.naive_utc();
            let (_,tick_end)=self.windows.window(dt).ok_or("Tick不在指定交易時段")?;
            if tick_end>end {break;}
            let tick=self.peek.take().unwrap();
            if tick_end<end {
                if self.proof.verified_bars>0 {return Err("Tick含K棒資料集中遺漏的時段，禁止跳過".into());}
                self.proof.skipped_before_first_bar+=1;
            } else {out.push(tick);}
        }
        if out.is_empty() {return Err(format!("{} 無對應Tick，不以OHLC替代",b.datetime));}
        let high=out.iter().map(|t|t.price).fold(f64::NEG_INFINITY,f64::max);
        let low=out.iter().map(|t|t.price).fold(f64::INFINITY,f64::min);
        let volume=out.iter().map(|t|t.volume as i128).sum::<i128>() as f64/1_000_000.0;
        if out[0].price!=b.open || high!=b.high || low!=b.low || out.last().unwrap().price!=b.close
            || (volume-b.volume).abs()>0.000001 {
            return Err(format!("{} Tick重建OHLCV與訊號K棒不符，停止回測",b.datetime));
        }
        self.proof.verified_bars+=1; self.proof.consumed_ticks+=out.len() as u64;
        Ok(out)
    }
    pub fn proof(&self)->TickExecutionProof {
        let mut p=self.proof.clone();p.verified_block_hashes_sha256=format!("{:x}",self.hashes.clone().finalize());p
    }
}
