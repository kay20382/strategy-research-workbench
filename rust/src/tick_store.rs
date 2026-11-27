//! Compact, lossless tick blocks in the existing market database. No deduplication.
use crate::{
    database::{self, Payload, Record},
    tick_bars::{decimal, text},
};
use chrono::{NaiveDate, NaiveDateTime};
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io::{BufReader, Read},
    path::Path,
};
const BLOCK: usize = 100_000;
const WIDTH: usize = 24;
const SCHEMA:&str="CREATE TABLE IF NOT EXISTS dataset_storage(dataset_id INTEGER PRIMARY KEY REFERENCES datasets(id),format TEXT NOT NULL CHECK(format='fixed6_le24_v1')) STRICT;
CREATE TABLE IF NOT EXISTS tick_blocks(dataset_id INTEGER NOT NULL REFERENCES datasets(id),block_no INTEGER NOT NULL,start_ns INTEGER NOT NULL,end_ns INTEGER NOT NULL,tick_count INTEGER NOT NULL,data BLOB NOT NULL,sha256 TEXT NOT NULL,PRIMARY KEY(dataset_id,block_no)) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS tick_blocks_time ON tick_blocks(dataset_id,start_ns,end_ns);
CREATE TABLE IF NOT EXISTS tick_canonical(symbol TEXT PRIMARY KEY,dataset_id INTEGER NOT NULL UNIQUE REFERENCES datasets(id)) STRICT;
CREATE TABLE IF NOT EXISTS tick_imports(id INTEGER PRIMARY KEY,dataset_id INTEGER NOT NULL REFERENCES datasets(id),source_format TEXT NOT NULL,source_path TEXT NOT NULL,source_sha256 TEXT NOT NULL,row_count INTEGER NOT NULL,first_ns INTEGER NOT NULL,last_ns INTEGER NOT NULL,UNIQUE(dataset_id,source_sha256)) STRICT;";
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
fn dt(s: &str) -> Result<NaiveDateTime, String> {
    for f in [
        "%Y/%m/%d %H:%M:%S%.f",
        "%Y-%m-%dT%H:%M:%S%.f",
        "%Y-%m-%d %H:%M:%S%.f",
    ] {
        if let Ok(d) = NaiveDateTime::parse_from_str(s, f) {
            return Ok(d);
        }
    }
    Err(format!("無效時間：{s}"))
}
fn iso(ns: i64) -> Result<String, String> {
    let seconds = ns.div_euclid(1_000_000_000);
    let fraction = ns.rem_euclid(1_000_000_000) as u32;
    Ok(chrono::DateTime::from_timestamp(seconds, fraction)
        .ok_or("時間超出範圍")?
        .naive_utc()
        .format("%Y-%m-%dT%H:%M:%S%.9f")
        .to_string())
}
fn value(bytes: &[u8]) -> i64 {
    i64::from_le_bytes(bytes.try_into().unwrap())
}

/// Append without using the historical importer's interval replacement policy.
pub(crate) const LIVE_OUT_OF_ORDER: &str = "成交時間倒退，無法追加至正式 Tick";

pub(crate) fn append_live(
    tx: &rusqlite::Transaction<'_>, id: i64, dt: NaiveDateTime, price: i64, volume: i64,
) -> Result<(), String> {
    let ns = dt.and_utc().timestamp_nanos_opt().ok_or("Tick 時間超出範圍")?;
    let tail: Option<(i64,i64,i64,Vec<u8>)> = tx.query_row(
        "SELECT block_no,start_ns,end_ns,data FROM tick_blocks WHERE dataset_id=?1 ORDER BY block_no DESC LIMIT 1",
        [id], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?)),
    ).optional().map_err(|e| e.to_string())?;
    let (block, start, mut data) = match tail {
        Some((_, _, end, _)) if ns < end => return Err(LIVE_OUT_OF_ORDER.into()),
        Some((block, start, _, data)) if data.len() < 256 * WIDTH => (block,start,data),
        Some((block, _, _, _)) => (block+1,ns,Vec::new()),
        None => (0,ns,Vec::new()),
    };
    let mut digest = Sha256::new();
    let old_hash: Option<String> = tx.query_row("SELECT sha256 FROM datasets WHERE id=?1",[id],|r|r.get(0))
        .map_err(|e|e.to_string())?;
    digest.update(old_hash.unwrap_or_default());
    for n in [ns,price,volume] {
        data.extend_from_slice(&n.to_le_bytes());
        digest.update(n.to_le_bytes());
    }
    let hash = format!("{:x}", Sha256::digest(&data));
    tx.execute("INSERT INTO tick_blocks VALUES(?1,?2,?3,?4,?5,?6,?7)
        ON CONFLICT(dataset_id,block_no) DO UPDATE SET end_ns=excluded.end_ns,tick_count=excluded.tick_count,data=excluded.data,sha256=excluded.sha256",
        params![id,block,start,ns,(data.len()/WIDTH) as i64,data,hash]).map_err(|e|e.to_string())?;
    let time = iso(ns)?;
    tx.execute("UPDATE datasets SET row_count=row_count+1,first_datetime=COALESCE(first_datetime,?2),last_datetime=?2,sha256=?3 WHERE id=?1",
        params![id,time,format!("{:x}",digest.finalize())]).map_err(|e|e.to_string())?;
    Ok(())
}

pub(crate) fn ensure_live_schema(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(SCHEMA)
}
#[derive(Serialize)]
pub struct ImportReport {
    pub dataset_id: i64,
    pub storage_format: String,
    pub source_sha256: String,
    pub count: u64,
    pub blocks: u64,
    pub first: String,
    pub last: String,
    pub total_volume: String,
    pub equal_timestamp_ticks: u64,
    pub consecutive_identical_ticks: u64,
    pub zero_volume_ticks: u64,
    pub zero_price_ticks: u64,
    pub skipped_rows: u64,
    pub timezone: String,
    pub dataset_total_count: u64,
    pub source_format: String,
    pub replaced_count: i64,
    pub overwrite_policy: String,
}
pub fn import(database: &Path, input: &Path, symbol: &str) -> Result<ImportReport, String> {
    import_format(database, input, symbol, "mc_csv")
}
pub fn import_format(
    database: &Path,
    input: &Path,
    symbol: &str,
    source_format: &str,
) -> Result<ImportReport, String> {
    let mut c = database::open(database)?;
    import_into(&mut c, input, symbol, source_format, "Asia/Taipei")
}
pub fn import_into(
    c: &mut Connection,
    input: &Path,
    symbol: &str,
    source_format: &str,
    timezone: &str,
) -> Result<ImportReport, String> {
    import_impl(c,input,symbol,source_format,timezone,true)
}
/// 新檔起訖時間的閉區間整段取代；新檔內合法相同成交完整保留。
pub fn upsert_into(c:&mut Connection,input:&Path,symbol:&str,source_format:&str,timezone:&str)->Result<ImportReport,String>{
    import_impl(c,input,symbol,source_format,timezone,true)
}
fn import_impl(
    c:&mut Connection,input:&Path,symbol:&str,source_format:&str,timezone:&str,overwrite:bool,
) -> Result<ImportReport,String> {
    if symbol.trim().is_empty() || timezone.trim().is_empty() {
        return Err("商品及時區標記不得為空".into());
    }
    if !["mc_csv", "taifex_normalized_csv", "normalized_csv"].contains(&source_format) {
        return Err("Tick 來源格式未支援".into());
    }
    c.execute_batch(SCHEMA).map_err(|e| e.to_string())?;
    let tx = c.transaction().map_err(|e| e.to_string())?;
    let existing_zone:Option<String>=tx.query_row("SELECT d.timezone FROM tick_canonical t JOIN datasets d ON d.id=t.dataset_id WHERE t.symbol=?1",[symbol],|r|r.get(0)).optional().map_err(|e|e.to_string())?;
    if existing_zone.as_ref().is_some_and(|z| z != timezone) {
        return Err("同一商品的時區標記不一致，不合併 Tick".into());
    }
    let existing:Option<(i64,i64,String,String,String)>=tx.query_row("SELECT d.id,d.row_count,d.last_datetime,d.first_datetime,d.sha256 FROM tick_canonical t JOIN datasets d ON d.id=t.dataset_id WHERE t.symbol=?1",[symbol],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?))).optional().map_err(|e|e.to_string())?;
    let (id, old_count, old_last, old_first, old_hash) = if let Some((id, n, last, first, hash)) =
        existing
    {
        (
            id,
            n,
            Some(
                dt(&last)?
                    .and_utc()
                    .timestamp_nanos_opt()
                    .ok_or("既有時間無效")?,
            ),
            Some(first),
            Some(hash),
        )
    } else {
        tx.execute("INSERT INTO datasets(symbol,source,kind,timezone,timestamp_role,path) VALUES(?1,'canonical_tick','trade',?3,'event',?2)",params![symbol,input.to_string_lossy(),timezone]).map_err(|e|e.to_string())?;
        let id = tx.last_insert_rowid();
        tx.execute(
            "INSERT INTO tick_canonical VALUES(?1,?2)",
            params![symbol, id],
        )
        .map_err(|e| e.to_string())?;
        tx.execute(
            "INSERT INTO dataset_storage VALUES(?1,'fixed6_le24_v1')",
            [id],
        )
        .map_err(|e| e.to_string())?;
        (id, 0, None, None, None)
    };
    if overwrite {tx.execute_batch("DROP TABLE IF EXISTS temp.incoming_tick_blocks; CREATE TEMP TABLE incoming_tick_blocks(dataset_id INTEGER,block_no INTEGER PRIMARY KEY,start_ns INTEGER,end_ns INTEGER,tick_count INTEGER,data BLOB,sha256 TEXT);").map_err(|e|e.to_string())?;}
    let mut insert = tx
        .prepare(if overwrite{"INSERT INTO incoming_tick_blocks VALUES(?1,?2,?3,?4,?5,?6,?7)"}else{"INSERT INTO tick_blocks VALUES(?1,?2,?3,?4,?5,?6,?7)"})
        .map_err(|e| e.to_string())?;
    let source = Hashed {
        reader: BufReader::with_capacity(
            4 * 1024 * 1024,
            File::open(input).map_err(|e| e.to_string())?,
        ),
        hash: Sha256::new(),
    };
    let delimiter = crate::market_csv::delimiter(input)?;
    let mut reader = crate::market_csv::TextCsv::new(source,delimiter)?;
    let mut h = reader
        .headers()
        .map_err(|e| e.to_string())?
        .iter()
        .map(crate::database::normalized)
        .collect::<Vec<_>>();
    if !h.iter().any(|s|s=="time"||s=="datetime") {if let Some(i)=h.iter().position(|s|s=="date"){h[i]="datetime".into();}}
    if h.iter().collect::<std::collections::HashSet<_>>().len() != h.len() {
        return Err("Tick 欄位重複".into());
    }
    let col = |s: &str| {
        h.iter()
            .position(|x| x == s)
            .ok_or_else(|| format!("缺少欄位：{s}"))
    };
    let datetime = h.iter().position(|x| x == "datetime");
    let date = if datetime.is_none() {
        Some(col("date")?)
    } else {
        None
    };
    let time = if datetime.is_none() {
        Some(col("time")?)
    } else {
        None
    };
    if !h.iter().any(|s|s=="price") && ["open","high","low","close"].iter().all(|name|h.iter().any(|s|s==name)) {
        return Err("這是OHLC K棒資料，請選1分K或日K，不可當成Tick匯入；未修改既有行情".into());
    }
    let price = col("price")?;
    let volume = col("volume")?;
    let mut data = Vec::with_capacity(BLOCK * WIDTH);
    let mut count = 0u64;
    let base_block: i64 = if overwrite {0}else{tx
        .query_row(
            "SELECT COALESCE(MAX(block_no)+1,0) FROM tick_blocks WHERE dataset_id=?1",
            [id],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?};
    let mut blocks = base_block as u64;
    let mut first_ns = 0;
    let mut last_ns = 0;
    let mut block_first = 0;
    let mut previous = None;
    let mut same = 0u64;
    let mut identical = 0u64;
    let mut zero_volume = 0u64;
    let mut zero_price = 0u64;
    let mut sum = 0i128;
    let mut cached_raw = String::new();
    let mut cached_ns = 0;
    for row in reader.records() {
        let row = row.map_err(|e| format!("第 {} 筆：{e}", count + 1))?;
        let mut raw = if let Some(i) = datetime {
            row[i].to_string()
        } else {
            format!("{} {}", &row[date.unwrap()], &row[time.unwrap()])
        };
        if let Some(i)=h.iter().position(|s|s=="nanosecond") {
            let fraction=row[i].parse::<u32>().map_err(|_|"Nanosecond欄須為0至999999999的整數")?;
            if fraction>=1_000_000_000 || raw.contains('.') {return Err("奈秒欄位不合法，或Time已含小數秒，不能重複指定".into());}
            raw.push_str(&format!(".{fraction:09}"));
        }
        let ns = if raw == cached_raw {
            cached_ns
        } else {
            let d = dt(&raw)?;
            if chrono::Timelike::nanosecond(&d) >= 1_000_000_000 {
                return Err("不支援閏秒時間戳記".into());
            }
            let n = d
                .and_utc()
                .timestamp_nanos_opt()
                .ok_or("時間超出奈秒整數範圍")?;
            cached_raw = raw;
            cached_ns = n;
            n
        };
        let px = i64::try_from(decimal(&row[price])?).map_err(|_| "成交價超出儲存範圍")?;
        let vol = i64::try_from(decimal(&row[volume])?).map_err(|_| "成交量超出儲存範圍")?;
        if vol < 0 {
            return Err(format!("第 {} 筆成交量為負", count + 1));
        }
        if !overwrite && count == 0 && old_last.is_some_and(|last| ns <= last) {
            return Err("此商品匯入區間與標準 Tick 資料重疊；取消整批，不以同秒價量去重，也不建立第二份資料".into());
        }
        if let Some((p, pp, pv)) = previous {
            if ns < p {
                return Err(format!("第 {} 筆時間倒退；取消整批匯入", count + 1));
            }
            if ns == p {
                same += 1;
                if px == pp && vol == pv {
                    identical += 1
                }
            }
        }
        previous = Some((ns, px, vol));
        sum = sum.checked_add(vol as i128).ok_or("總成交量溢位")?;
        zero_volume += (vol == 0) as u64;
        zero_price += (px == 0) as u64;
        if count == 0 {
            first_ns = ns
        }
        last_ns = ns;
        if data.is_empty() {
            block_first = ns
        }
        data.extend_from_slice(&ns.to_le_bytes());
        data.extend_from_slice(&px.to_le_bytes());
        data.extend_from_slice(&vol.to_le_bytes());
        count += 1;
        if data.len() == BLOCK * WIDTH {
            insert
                .execute(params![
                    id,
                    blocks as i64,
                    block_first,
                    ns,
                    BLOCK as i64,
                    &data,
                    format!("{:x}", Sha256::digest(&data))
                ])
                .map_err(|e| e.to_string())?;
            blocks += 1;
            data.clear();
        }
        if count % 10_000_000 == 0 {
            eprintln!("行情資料庫已暫存 {count} 筆 Tick（交易提交前不列為完成）")
        }
    }
    if count == 0 {
        return Err("Tick 檔沒有資料".into());
    }
    if !data.is_empty() {
        insert
            .execute(params![
                id,
                blocks as i64,
                block_first,
                last_ns,
                (data.len() / WIDTH) as i64,
                &data,
                format!("{:x}", Sha256::digest(&data))
            ])
            .map_err(|e| e.to_string())?;
        blocks += 1
    }
    drop(insert);
    let removed=if overwrite{merge_incoming(&tx,id,first_ns,last_ns)?}else{0};
    let hash = format!("{:x}", reader.into_inner().hash.finalize());
    let first = iso(first_ns)?;
    let last = iso(last_ns)?;
    let canonical_hash = old_hash.map_or_else(
        || hash.clone(),
        |h| {
            format!(
                "{:x}",
                Sha256::digest(format!("tick-lineage-v1:{h}:{hash}").as_bytes())
            )
        },
    );
    let canonical_first = old_first.map_or_else(||first.clone(),|old|old.min(first.clone()));
    let canonical_last = old_last.map_or_else(||Ok(last.clone()),|old|iso(old.max(last_ns)))?;
    let import_sql=if overwrite{"INSERT INTO tick_imports(dataset_id,source_format,source_path,source_sha256,row_count,first_ns,last_ns) VALUES(?1,?2,?3,?4,?5,?6,?7) ON CONFLICT(dataset_id,source_sha256) DO UPDATE SET source_path=excluded.source_path,row_count=excluded.row_count"}else{"INSERT INTO tick_imports(dataset_id,source_format,source_path,source_sha256,row_count,first_ns,last_ns) VALUES(?1,?2,?3,?4,?5,?6,?7)"};
    tx.execute(import_sql,params![id,source_format,input.to_string_lossy(),hash,count as i64,first_ns,last_ns]).map_err(|e|e.to_string())?;
    if overwrite{
        tx.execute_batch("CREATE TABLE IF NOT EXISTS tick_upsert_events(id INTEGER PRIMARY KEY,dataset_id INTEGER NOT NULL,source_path TEXT NOT NULL,sha256 TEXT NOT NULL,incoming_rows INTEGER NOT NULL,replaced_rows INTEGER NOT NULL,imported_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP)").map_err(|e|e.to_string())?;
        tx.execute("INSERT INTO tick_upsert_events(dataset_id,source_path,sha256,incoming_rows,replaced_rows) VALUES(?1,?2,?3,?4,?5)",params![id,input.to_string_lossy(),hash,count as i64,removed]).map_err(|e|e.to_string())?;
    }
    tx.execute(
        "UPDATE datasets SET sha256=?1,row_count=?2,first_datetime=?3,last_datetime=?4 WHERE id=?5",
        params![
            canonical_hash,
            old_count + count as i64 - removed,
            canonical_first,
            canonical_last,
            id
        ],
    )
    .map_err(|e| format!("匯入重複或更新失敗；整批取消：{e}"))?;
    tx.commit().map_err(|e| e.to_string())?;
    Ok(ImportReport {
        dataset_id: id,
        storage_format: "fixed6_le24_v1".into(),
        source_sha256: hash,
        count,
        blocks: blocks - base_block as u64,
        first,
        last,
        total_volume: text(sum),
        equal_timestamp_ticks: same,
        consecutive_identical_ticks: identical,
        zero_volume_ticks: zero_volume,
        zero_price_ticks: zero_price,
        skipped_rows: 0,
        timezone: timezone.into(),
        dataset_total_count: (old_count + count as i64 - removed) as u64,
        source_format: source_format.into(),
        replaced_count: removed,
        overwrite_policy: "新檔開始至結束時間閉區間整段覆蓋；同批合法相同成交保留".into(),
    })
}
// 一次最多保留兩個來源區塊與一個輸出區塊；不把億筆 Tick 展開到記憶體。
fn merge_incoming(c:&Connection,id:i64,first:i64,last:i64)->Result<i64,String>{
    struct Cursor<'a>{rows:rusqlite::Rows<'a>,data:Vec<u8>,offset:usize}
    impl Cursor<'_>{fn next(&mut self)->Result<Option<[i64;3]>,String>{
        if self.offset==self.data.len(){
            let Some(r)=self.rows.next().map_err(|e|e.to_string())? else{return Ok(None)};
            let data:Vec<u8>=r.get(0).map_err(|e|e.to_string())?;let hash:String=r.get(1).map_err(|e|e.to_string())?;let count:i64=r.get(2).map_err(|e|e.to_string())?;
            if count<=0||data.len()!=count as usize*WIDTH||format!("{:x}",Sha256::digest(&data))!=hash{return Err("Tick 區塊長度或雜湊不符，取消覆蓋".into());}
            self.data=data;self.offset=0;
        }
        let b=&self.data[self.offset..self.offset+WIDTH];self.offset+=WIDTH;Ok(Some([value(&b[..8]),value(&b[8..16]),value(&b[16..24])]))
    }}
    let affected:(Option<i64>,Option<i64>)=c.query_row("SELECT MIN(block_no),MAX(block_no) FROM tick_blocks WHERE dataset_id=?1 AND end_ns>=?2 AND start_ns<=?3",params![id,first,last],|r|Ok((r.get(0)?,r.get(1)?))).map_err(|e|e.to_string())?;
    let start=if let Some(s)=affected.0{s}else{c.query_row("SELECT COALESCE(MIN(block_no),(SELECT COUNT(*) FROM tick_blocks WHERE dataset_id=?1)) FROM tick_blocks WHERE dataset_id=?1 AND start_ns>?2",params![id,last],|r|r.get::<_,i64>(0)).map_err(|e|e.to_string())?};
    let end=affected.1.unwrap_or(start-1);
    c.execute_batch("DROP TABLE IF EXISTS temp.merged_tick_blocks; CREATE TEMP TABLE merged_tick_blocks(block_no INTEGER PRIMARY KEY,start_ns INTEGER,end_ns INTEGER,tick_count INTEGER,data BLOB,sha256 TEXT);").map_err(|e|e.to_string())?;
    let mut old_stmt=c.prepare("SELECT data,sha256,tick_count FROM tick_blocks WHERE dataset_id=?1 AND block_no>=?2 AND block_no<=?3 ORDER BY block_no").map_err(|e|e.to_string())?;
    let mut new_stmt=c.prepare("SELECT data,sha256,tick_count FROM incoming_tick_blocks ORDER BY block_no").map_err(|e|e.to_string())?;
    let mut old=Cursor{rows:old_stmt.query(params![id,start,end]).map_err(|e|e.to_string())?,data:vec![],offset:0};
    let mut incoming=Cursor{rows:new_stmt.query([]).map_err(|e|e.to_string())?,data:vec![],offset:0};
    let mut a=old.next()?;let mut b=incoming.next()?;let mut removed=0;let mut buffer=Vec::with_capacity(BLOCK*WIDTH);let mut blocks=0i64;let mut lo=0i64;let mut hi=0i64;
    let mut flush=|buffer:&mut Vec<u8>,lo:i64,hi:i64|->Result<(),String>{
        if !buffer.is_empty(){c.execute("INSERT INTO merged_tick_blocks VALUES(?1,?2,?3,?4,?5,?6)",params![blocks,lo,hi,(buffer.len()/WIDTH) as i64,buffer.as_slice(),format!("{:x}",Sha256::digest(buffer.as_slice()))]).map_err(|e|e.to_string())?;blocks+=1;buffer.clear();}Ok(())
    };
    while a.is_some()||b.is_some(){
        if a.is_some_and(|x|x[0]>=first&&x[0]<=last){removed+=1;a=old.next()?;continue;}
        let take_old=match(a,b){(Some(x),Some(y))=>x[0]<y[0],(Some(_),None)=>true,_=>false};
        let row=if take_old{let r=a.unwrap();a=old.next()?;r}else{let r=b.unwrap();b=incoming.next()?;r};
        if buffer.is_empty(){lo=row[0];}hi=row[0];for v in row{buffer.extend(v.to_le_bytes());}if buffer.len()==BLOCK*WIDTH{flush(&mut buffer,lo,hi)?;}
    }
    flush(&mut buffer,lo,hi)?;drop(flush);drop(old);drop(incoming);drop(old_stmt);drop(new_stmt);
    c.execute("DELETE FROM tick_blocks WHERE dataset_id=?1 AND block_no>=?2 AND block_no<=?3",params![id,start,end]).map_err(|e|e.to_string())?;
    let delta=blocks-(end-start+1);
    // 先映射負值，避免更新主鍵時與尚未搬移的區塊碰撞。
    c.execute("UPDATE tick_blocks SET block_no=-block_no-1 WHERE dataset_id=?1 AND block_no>?2",params![id,end]).map_err(|e|e.to_string())?;
    c.execute("UPDATE tick_blocks SET block_no=-block_no-1+?2 WHERE dataset_id=?1 AND block_no<0",params![id,delta]).map_err(|e|e.to_string())?;
    c.execute("INSERT INTO tick_blocks SELECT ?1,block_no+?2,start_ns,end_ns,tick_count,data,sha256 FROM merged_tick_blocks ORDER BY block_no",params![id,start]).map_err(|e|e.to_string())?;
    Ok(removed)
}
pub fn contains(c: &Connection, id: i64) -> Result<bool, String> {
    let table:bool=c.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='dataset_storage')",[],|r|r.get(0)).map_err(|e|e.to_string())?;
    if !table {
        return Ok(false);
    }
    c.query_row("SELECT EXISTS(SELECT 1 FROM dataset_storage WHERE dataset_id=?1 AND format='fixed6_le24_v1')",[id],|r|r.get(0)).map_err(|e|e.to_string())
}

pub(crate) fn recent(c: &Connection, id: i64, limit: usize, bounds: (i64, i64)) -> Result<Vec<[i64; 3]>, String> {
    let (lo, hi) = bounds;
    let mut stmt = c.prepare("SELECT data,sha256,tick_count FROM tick_blocks WHERE dataset_id=?1 AND end_ns>=?2 AND start_ns<=?3 ORDER BY block_no DESC")
        .map_err(|e| e.to_string())?;
    let mut rows = stmt.query(params![id, lo, hi]).map_err(|e| e.to_string())?;
    let mut ticks = Vec::new();
    while ticks.len() < limit {
        let Some(row) = rows.next().map_err(|e| e.to_string())? else { break };
        let (data, hash, count): (Vec<u8>, String, i64) =
            (|| -> rusqlite::Result<_> { Ok((row.get(0)?, row.get(1)?, row.get(2)?)) })()
                .map_err(|e| e.to_string())?;
        if count <= 0 || usize::try_from(count).ok().and_then(|n| n.checked_mul(WIDTH)) != Some(data.len())
            || format!("{:x}", Sha256::digest(&data)) != hash
        {
            return Err("Tick 區塊長度或雜湊不符".into());
        }
        for tick in data.chunks_exact(WIDTH).rev() {
            let ns = value(&tick[..8]);
            if ns > hi { continue; }
            if ns < lo || ticks.len() == limit { break; }
            ticks.push([ns, value(&tick[8..16]), value(&tick[16..24])]);
        }
    }
    ticks.reverse();
    Ok(ticks)
}
/// 串流掃描指定期間的原始固定精度 Tick；每個讀取區塊均核對雜湊。
pub fn visit(c:&Connection,id:i64,from:&str,to:&str,mut consume:impl FnMut(i64,i64,i64)->Result<(),String>)->Result<usize,String>{
    let lo=bound(from,i64::MIN)?;let hi=bound(to,i64::MAX)?;
    let mut stmt=c.prepare("SELECT data,sha256,tick_count FROM tick_blocks WHERE dataset_id=?1 AND end_ns>=?2 AND start_ns<=?3 ORDER BY block_no").map_err(|e|e.to_string())?;
    let mut rows=stmt.query(params![id,lo,hi]).map_err(|e|e.to_string())?;let mut count=0;let mut previous=None;
    while let Some(r)=rows.next().map_err(|e|e.to_string())?{
        let data:Vec<u8>=r.get(0).map_err(|e|e.to_string())?;let hash:String=r.get(1).map_err(|e|e.to_string())?;let n:i64=r.get(2).map_err(|e|e.to_string())?;
        if n<=0||data.len()!=n as usize*WIDTH||format!("{:x}",Sha256::digest(&data))!=hash{return Err("Tick 區塊長度或雜湊不符".into());}
        for raw in data.chunks_exact(WIDTH){let ns=value(&raw[..8]);if ns<lo{continue;}if ns>hi{return Ok(count);}if previous.is_some_and(|p|ns<p){return Err("Tick 時間倒退".into());}previous=Some(ns);consume(ns,value(&raw[8..16]),value(&raw[16..24]))?;count+=1;}
    }Ok(count)
}
fn bound(s: &str, default: i64) -> Result<i64, String> {
    if s.is_empty() || s == "z" || s == "*" {
        return Ok(default);
    }
    let d = dt(s).or_else(|_| {
        NaiveDate::parse_from_str(s, "%Y-%m-%d")
            .map(|d| d.and_hms_opt(0, 0, 0).unwrap())
            .map_err(|e| e.to_string())
    })?;
    d.and_utc()
        .timestamp_nanos_opt()
        .ok_or("查詢時間超出範圍".into())
}

#[derive(Debug, Serialize)]
pub struct PositionedRecord {
    pub ordinal: i64,
    #[serde(flatten)]
    pub record: Record,
}
pub fn query_tick_page(
    conn: &Connection, id: i64, from: &str, to: &str, limit: usize, after: Option<(&str, i64)>,
) -> Result<Vec<PositionedRecord>, String> {
    if !(1..=100_000).contains(&limit) {
        return Err("查詢筆數須為 1 至 100000".into());
    }
    if contains(conn, id)? {
        return query_packed_page(conn, id, from, to, limit, after);
    }
    if let Some((time, ordinal)) = after {
        let found: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM records WHERE dataset_id=?1 AND ordinal=?2 AND datetime=?3)",
            params![id, ordinal, time], |row| row.get(0),
        ).map_err(|e| e.to_string())?;
        if !found { return Err("續查游標的時間戳或 ordinal 不符".into()); }
    }
    let (time, ordinal) = after.unwrap_or((from, i64::MIN));
    let mut statement = conn.prepare(
        "SELECT datetime,available_at,payload,ordinal FROM records
         WHERE dataset_id=?1 AND datetime>=?2 AND datetime<=?3 AND (datetime,ordinal)>(?4,?5)
         ORDER BY datetime,ordinal LIMIT ?6",
    ).map_err(|e| e.to_string())?;
    let rows = statement.query_map(params![id, from, to, time, ordinal, limit as i64], |row| {
        Ok((row.get::<_,String>(0)?,row.get::<_,Option<String>>(1)?,row.get::<_,String>(2)?,row.get::<_,i64>(3)?))
    }).map_err(|e| e.to_string())?;
    rows.map(|row| {
        let (datetime,available_at,payload,ordinal) = row.map_err(|e| e.to_string())?;
        let payload = serde_json::from_str(&payload).map_err(|e| e.to_string())?;
        Ok(PositionedRecord { ordinal, record: Record { datetime, available_at, payload } })
    }).collect()
}

fn query_packed_page(
    conn: &Connection, id: i64, from: &str, to: &str, limit: usize, after: Option<(&str, i64)>,
) -> Result<Vec<PositionedRecord>, String> {
    let after = after.map(|(time, ordinal)| Ok::<_,String>((bound(time, i64::MIN)?, ordinal))).transpose()?;
    let lo = after.map_or(bound(from, i64::MIN)?, |(time, _)| time);
    let hi = bound(to, i64::MAX)?;
    let mut metadata = conn.prepare(
        "SELECT block_no,tick_count FROM tick_blocks
         WHERE dataset_id=?1 AND end_ns>=?2 AND start_ns<=?3 ORDER BY block_no",
    ).map_err(|e| e.to_string())?;
    let mut blocks = metadata.query(params![id, lo, hi]).map_err(|e| e.to_string())?;
    let Some(first) = blocks.next().map_err(|e| e.to_string())? else {
        return if after.is_some() { Err("續查游標的時間戳或 ordinal 不符".into()) } else { Ok(Vec::new()) };
    };
    let first_block: i64 = first.get(0).map_err(|e| e.to_string())?;
    let mut ordinal: i64 = conn.query_row(
        "SELECT COALESCE(SUM(tick_count),0) FROM tick_blocks WHERE dataset_id=?1 AND block_no<?2",
        params![id, first_block], |row| row.get(0),
    ).map_err(|e| e.to_string())?;
    let mut current = Some(first);
    let mut found_cursor = after.is_none();
    let mut result = Vec::new();
    let mut block_data = conn.prepare("SELECT data,sha256 FROM tick_blocks WHERE dataset_id=?1 AND block_no=?2")
        .map_err(|e| e.to_string())?;
    while let Some(row) = current {
        let (block, count): (i64,i64) = (|| -> rusqlite::Result<_> { Ok((row.get(0)?, row.get(1)?)) })()
            .map_err(|e| e.to_string())?;
        let end_ordinal = ordinal.checked_add(count).filter(|_| count>0).ok_or("Tick 區塊筆數無效")?;
        if after.is_some_and(|(_, resume)| end_ordinal < resume) {
            ordinal = end_ordinal;
            current = blocks.next().map_err(|e| e.to_string())?;
            continue;
        }
        let (data, hash): (Vec<u8>,String) = block_data.query_row(params![id, block], |row| Ok((row.get(0)?,row.get(1)?)))
            .map_err(|e| e.to_string())?;
        if usize::try_from(count).ok().and_then(|n| n.checked_mul(WIDTH)) != Some(data.len())
            || format!("{:x}", Sha256::digest(&data)) != hash {
            return Err("Tick 區塊長度或雜湊不符".into());
        }
        if !found_cursor {
            let (time, resume) = after.expect("resume position");
            if resume <= ordinal || resume > end_ordinal
                || value(&data[((resume-ordinal-1) as usize)*WIDTH..][..8]) != time {
                return Err("續查游標的時間戳或 ordinal 不符".into());
            }
            found_cursor = true;
        }
        let skip = after.map_or(0, |(_, resume)| resume.saturating_sub(ordinal).max(0)) as usize;
        for (index, raw) in data.chunks_exact(WIDTH).enumerate().skip(skip) {
            let ns = value(&raw[..8]);
            if ns < lo { continue; }
            if ns > hi { return Ok(result); }
            let record = Record { datetime: iso(ns)?, available_at: None, payload: Payload::Trade {
                price: text(value(&raw[8..16]).into()), volume: text(value(&raw[16..24]).into()),
            }};
            result.push(PositionedRecord { ordinal: ordinal + index as i64 + 1, record });
            if result.len() == limit { return Ok(result); }
        }
        ordinal = end_ordinal;
        current = blocks.next().map_err(|e| e.to_string())?;
    }
    if found_cursor { Ok(result) } else { Err("續查游標的時間戳或 ordinal 不符".into()) }
}
pub fn query(
    c: &Connection,
    id: i64,
    from: &str,
    to: &str,
    limit: usize,
) -> Result<Vec<Record>, String> {
    if limit == 0 || limit > 100_000 {
        return Err("查詢筆數須為1至100000".into());
    }
    let lo = bound(from, i64::MIN)?;
    let hi = bound(to, i64::MAX)?;
    if lo > hi {
        return Err("查詢起點晚於終點".into());
    }
    let mut stmt=c.prepare("SELECT data,sha256,tick_count FROM tick_blocks WHERE dataset_id=?1 AND end_ns>=?2 AND start_ns<=?3 ORDER BY block_no").map_err(|e|e.to_string())?;
    let blocks = stmt
        .query_map(params![id, lo, hi], |r| {
            Ok((
                r.get::<_, Vec<u8>>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, i64>(2)?,
            ))
        })
        .map_err(|e| e.to_string())?;
    let mut out = vec![];
    for b in blocks {
        let (data, hash, n) = b.map_err(|e| e.to_string())?;
        if data.len() != n as usize * WIDTH || format!("{:x}", Sha256::digest(&data)) != hash {
            return Err("Tick 區塊長度或雜湊不符".into());
        }
        for t in data.chunks_exact(WIDTH) {
            let ns = value(&t[..8]);
            if ns < lo {
                continue;
            }
            if ns > hi {
                return Ok(out);
            }
            out.push(Record {
                datetime: iso(ns)?,
                available_at: None,
                payload: Payload::Trade {
                    price: text(value(&t[8..16]) as i128),
                    volume: text(value(&t[16..24]) as i128),
                },
            });
            if out.len() == limit {
                return Ok(out);
            }
        }
    }
    Ok(out)
}
#[derive(Serialize)]
pub struct Verification {
    pub dataset_id: i64,
    pub passed: bool,
    pub count: u64,
    pub blocks: u64,
    pub total_volume: String,
    pub first: String,
    pub last: String,
}
pub fn verify(c: &Connection, id: i64) -> Result<Verification, String> {
    let expected: i64 = c
        .query_row("SELECT row_count FROM datasets WHERE id=?1", [id], |r| {
            r.get(0)
        })
        .map_err(|e| e.to_string())?;
    let mut stmt=c.prepare("SELECT block_no,start_ns,end_ns,tick_count,data,sha256 FROM tick_blocks WHERE dataset_id=?1 ORDER BY block_no").map_err(|e|e.to_string())?;
    let mut rows = stmt.query([id]).map_err(|e| e.to_string())?;
    let mut count = 0u64;
    let mut blocks = 0u64;
    let mut sum = 0i128;
    let mut first = None;
    let mut last = None;
    while let Some(row) = rows.next().map_err(|e| e.to_string())? {
        let number: i64 = row.get(0).map_err(|e| e.to_string())?;
        let start: i64 = row.get(1).map_err(|e| e.to_string())?;
        let end: i64 = row.get(2).map_err(|e| e.to_string())?;
        let n: i64 = row.get(3).map_err(|e| e.to_string())?;
        let data: Vec<u8> = row.get(4).map_err(|e| e.to_string())?;
        let hash: String = row.get(5).map_err(|e| e.to_string())?;
        if number != blocks as i64
            || n <= 0
            || data.len() != n as usize * WIDTH
            || format!("{:x}", Sha256::digest(&data)) != hash
        {
            return Err("Tick 區塊順序、長度或雜湊錯誤".into());
        }
        if value(&data[..8]) != start
            || value(&data[data.len() - WIDTH..data.len() - WIDTH + 8]) != end
        {
            return Err("Tick 區塊時間索引不符".into());
        }
        for t in data.chunks_exact(WIDTH) {
            let ns = value(&t[..8]);
            if last.is_some_and(|p| ns < p) {
                return Err("Tick 資料庫時間倒退".into());
            }
            first.get_or_insert(ns);
            last = Some(ns);
            let v = value(&t[16..24]);
            if v < 0 {
                return Err("Tick 資料庫成交量為負".into());
            }
            sum = sum.checked_add(v as i128).ok_or("成交量溢位")?;
            count += 1
        }
        blocks += 1;
        if blocks % 200 == 0 {
            eprintln!("已回讀驗證 {count} 筆 Tick")
        }
    }
    if count as i64 != expected || count == 0 {
        return Err("資料庫摘要與實際 Tick 筆數不符".into());
    }
    Ok(Verification {
        dataset_id: id,
        passed: true,
        count,
        blocks,
        total_volume: text(sum),
        first: iso(first.unwrap())?,
        last: iso(last.unwrap())?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn formats_share_one_dataset_keep_identical_trades_and_replace_overlap() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("artifacts")
            .join(format!(
                "tick-store-test-{}",
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
        std::fs::create_dir_all(&root).unwrap();
        let mc = root.join("mc.csv");
        let taifex = root.join("normalized.csv");
        let bad = root.join("bad.csv");
        std::fs::write(
            &mc,
            "Date,Time,Price,Volume\n2026/10/22,15:00:00,100,1\n2026/10/22,15:00:00,100,1\n",
        )
        .unwrap();
        std::fs::write(
            &taifex,
            "datetime|price|volume\n2026-10-22T15:00:01|101|2\n",
        )
        .unwrap();
        std::fs::write(
            &bad,
            "datetime,price,volume\n2026-10-22T15:00:02,102,1\n2026-10-22T15:00:03,103,-1\n",
        )
        .unwrap();
        let mut c = database::open(&root.join("test.sqlite")).unwrap();
        let a = import_into(&mut c, &mc, "TXF1", "mc_csv", "Asia/Taipei").unwrap();
        let b = import_into(
            &mut c,
            &taifex,
            "TXF1",
            "taifex_normalized_csv",
            "Asia/Taipei",
        )
        .unwrap();
        assert_eq!(a.dataset_id, b.dataset_id);
        assert_eq!(b.dataset_total_count, 3);
        let rows = database::query(&c, a.dataset_id, "", "z", 100).unwrap();
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].datetime, rows[1].datetime);
        assert_eq!(import_into(&mut c, &mc, "TXF1", "mc_csv", "Asia/Taipei").unwrap().dataset_total_count,3);
        assert!(import_into(&mut c, &bad, "TXF1", "mc_csv", "Asia/Taipei").is_err());
        assert_eq!(verify(&c, a.dataset_id).unwrap().count, 3);
        assert_eq!(
            c.query_row("SELECT count(*) FROM tick_canonical", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            1
        );
        assert_eq!(
            c.query_row("SELECT count(*) FROM tick_imports", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            2
        );
        assert_eq!(
            c.query_row("SELECT count(*) FROM records", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            0
        );
        c.execute(
            "UPDATE tick_blocks SET data=zeroblob(length(data)) WHERE block_no=0",
            [],
        )
        .unwrap();
        assert!(verify(&c, a.dataset_id).is_err());
        assert!(database::query(&c, a.dataset_id, "", "z", 100).is_err());
    }
}
