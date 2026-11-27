//! 每商品 Tick／1分K／日K；原子匯入、同時間後蓋前。
use crate::{
    database::{self, Columns, DataKind},
    tick_store,
};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io::Read,
    path::Path,
};
const SCHEMA:&str="
CREATE TABLE IF NOT EXISTS market_sources(symbol TEXT COLLATE NOCASE NOT NULL,kind TEXT NOT NULL CHECK(kind IN ('tick','minute','daily')),dataset_id INTEGER UNIQUE NOT NULL REFERENCES datasets(id),PRIMARY KEY(symbol,kind)) STRICT;
CREATE TABLE IF NOT EXISTS market_bar_keys(dataset_id INTEGER NOT NULL REFERENCES datasets(id),time_key TEXT NOT NULL,ordinal INTEGER NOT NULL,PRIMARY KEY(dataset_id,time_key),UNIQUE(dataset_id,ordinal)) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS market_import_events(id INTEGER PRIMARY KEY,dataset_id INTEGER NOT NULL,source_path TEXT NOT NULL,source_sha256 TEXT NOT NULL,input_rows INTEGER NOT NULL,effective_rows INTEGER NOT NULL,replaced_rows INTEGER NOT NULL,imported_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP) STRICT;
";
pub fn initialize(root: &Path, path: &Path) -> Result<(), String> {
    let mut c = database::open(path)?;
    ensure(&mut c)?;
    let config = root.join("data/canonical-market-sources.json");
    if config.exists() {
        let v: Value = serde_json::from_reader(File::open(config).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
        for seed in v["legacy_base_sources"]
            .as_array()
            .ok_or("基礎資料來源設定無效")?
        {
            let id = seed["dataset_id"].as_i64().ok_or("基礎ID無效")?;
            let row: Option<(String, String, String)> = c
                .query_row(
                    "SELECT symbol,kind,path FROM datasets WHERE id=?1",
                    [id],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .optional()
                .map_err(|e| e.to_string())?;
            if let Some((symbol, kind, path)) = row {
                if seed["symbol"] == symbol
                    && seed["kind"] == kind
                    && Path::new(&path).file_name().is_some_and(|n| {
                        n.to_string_lossy() == seed["filename"].as_str().unwrap_or("")
                    })
                {
                    c.execute(
                        "INSERT OR IGNORE INTO market_sources VALUES(?1,?2,?3)",
                        params![symbol, kind, id],
                    )
                    .map_err(|e| e.to_string())?;
                }
            }
        }
    }
    Ok(())
}
fn ensure(c: &mut Connection) -> Result<(), String> {
    c.execute_batch(SCHEMA).map_err(|e| e.to_string())?;
    let ticks: bool = c
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='tick_canonical')",
            [],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    if ticks {
        c.execute("INSERT OR IGNORE INTO market_sources SELECT symbol,'tick',dataset_id FROM tick_canonical",[]).map_err(|e|e.to_string())?;
    }
    Ok(())
}
pub fn sources(path: &Path) -> Result<Value, String> {
    let c = database::open_existing(path)?;
    let mut stmt=c.prepare("SELECT m.symbol,m.kind,m.dataset_id,d.row_count,d.first_datetime,d.last_datetime,d.timezone,d.timestamp_role FROM market_sources m JOIN datasets d ON d.id=m.dataset_id ORDER BY m.symbol,m.kind").map_err(|e|e.to_string())?;
    let rows=stmt.query_map([],|r|Ok(json!({"symbol":r.get::<_,String>(0)?,"kind":r.get::<_,String>(1)?,"dataset_id":r.get::<_,i64>(2)?,"row_count":r.get::<_,i64>(3)?,"first_datetime":r.get::<_,Option<String>>(4)?,"last_datetime":r.get::<_,Option<String>>(5)?,"timezone":r.get::<_,String>(6)?,"timestamp_role":r.get::<_,String>(7)?}))).map_err(|e|e.to_string())?;
    Ok(json!(rows
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?))
}
pub fn resolve(path: &Path, symbol: &str, kind: &str) -> Result<i64, String> {
    let c = database::open_existing(path)?;
    resolve_in(&c, symbol, kind)
}

pub fn resolve_in(c: &Connection, symbol: &str, kind: &str) -> Result<i64, String> {
    c.query_row(
        "SELECT dataset_id FROM market_sources WHERE symbol=?1 COLLATE NOCASE AND kind=?2",
        params![symbol, kind],
        |r| r.get(0),
    )
    .optional()
    .map_err(|e| e.to_string())?
    .ok_or_else(|| format!("商品 {symbol} 尚未匯入 {kind} 基礎資料"))
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
pub fn import(
    path: &Path,
    file: &Path,
    symbol: &str,
    kind: &str,
    timezone: &str,
    role: &str,
) -> Result<Value, String> {
    let symbol = symbol.trim().to_uppercase();
    if symbol.is_empty() || timezone.trim().is_empty() {
        return Err("商品與時區不得空白；未知時區填 unspecified".into());
    }
    if !["tick", "minute", "daily"].contains(&kind) {
        return Err("僅接受 Tick、1分K、日K".into());
    }
    crate::market_csv::check_kind(file,kind)?;
    let mut c = database::open(path)?;
    ensure(&mut c)?;
    if kind == "tick" {
        if role != "event" {
            return Err("Tick 時間標記須為 event".into());
        }
        let report = tick_store::upsert_into(&mut c, file, &symbol, "normalized_csv", timezone)?;
        c.execute(
            "INSERT OR IGNORE INTO market_sources VALUES(?1,'tick',?2)",
            params![symbol, report.dataset_id],
        )
        .map_err(|e| e.to_string())?;
        return Ok(
            json!({"kind":"tick","symbol":symbol,"dataset_id":report.dataset_id,"input_rows":report.count,"total_rows":report.dataset_total_count,"first":report.first,"last":report.last,"source_sha256":report.source_sha256,"conflict":"新檔起訖時間閉區間整段取代舊Tick，新檔內多筆完整保留"}),
        );
    }
    if !["start", "end", "unspecified"].contains(&role) {
        return Err("K棒時間標記須為 start、end 或 unspecified".into());
    }
    let kind_enum = if kind == "minute" {
        DataKind::Minute
    } else {
        DataKind::Daily
    };
    let delimiter = crate::market_csv::delimiter(file)?;
    let mut reader = crate::market_csv::TextCsv::new(Hashed {
            reader: File::open(file).map_err(|e| e.to_string())?,
            hash: Sha256::new(),
        },delimiter)?;
    let columns = Columns::new(reader.headers().map_err(|e| e.to_string())?, kind_enum)?;
    let tx = c.transaction().map_err(|e| e.to_string())?;
    let existing:Option<(i64,String,String,String)>=tx.query_row("SELECT d.id,d.timezone,d.timestamp_role,COALESCE(d.sha256,'') FROM market_sources m JOIN datasets d ON d.id=m.dataset_id WHERE m.symbol=?1 COLLATE NOCASE AND m.kind=?2",params![symbol,kind],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional().map_err(|e|e.to_string())?;
    let (id, oldhash) = if let Some((id, zone, oldrole, hash)) = existing {
        if zone != timezone || oldrole != role {
            return Err(format!(
                "此商品既有 {kind} 的時區／時間標記為 {zone}／{oldrole}，請先統一標記，未混合覆蓋"
            ));
        }
        (id, hash)
    } else {
        tx.execute("INSERT INTO datasets(symbol,source,kind,timezone,timestamp_role,path) VALUES(?1,'canonical_base',?2,?3,?4,?5)",params![symbol,kind,timezone,role,file.to_string_lossy()]).map_err(|e|e.to_string())?;
        let id = tx.last_insert_rowid();
        tx.execute(
            "INSERT INTO market_sources VALUES(?1,?2,?3)",
            params![symbol, kind, id],
        )
        .map_err(|e| e.to_string())?;
        (id, String::new())
    };
    tx.execute("INSERT OR IGNORE INTO market_bar_keys SELECT dataset_id,CASE WHEN ?2='daily' THEN substr(datetime,1,10) ELSE datetime END,ordinal FROM records WHERE dataset_id=?1",params![id,kind]).map_err(|e|e.to_string())?;
    let consistent:bool=tx.query_row("SELECT (SELECT COUNT(*) FROM market_bar_keys WHERE dataset_id=?1)=(SELECT COUNT(*) FROM records WHERE dataset_id=?1)",[id],|r|r.get(0)).map_err(|e|e.to_string())?;
    if !consistent {
        return Err("既有基礎資料有重複時間，未自動刪除，請先檢查來源".into());
    }
    tx.execute_batch("DROP TABLE IF EXISTS temp.incoming_base_bars; CREATE TEMP TABLE incoming_base_bars(time_key TEXT PRIMARY KEY,datetime TEXT NOT NULL,available_at TEXT,payload TEXT NOT NULL);").map_err(|e|e.to_string())?;
    let mut count = 0i64;
    {
        let mut stmt=tx.prepare("INSERT INTO incoming_base_bars VALUES(?1,?2,?3,?4) ON CONFLICT(time_key) DO UPDATE SET datetime=excluded.datetime,available_at=excluded.available_at,payload=excluded.payload").map_err(|e|e.to_string())?;
        for row in reader.records() {
            let row = row.map_err(|e| e.to_string())?;
            let record = columns
                .record(&row, kind_enum)
                .map_err(|e| format!("第 {} 列：{e}", count + 2))?;
            let key = if kind == "daily" {
                record.datetime[..10].to_owned()
            } else {
                record.datetime.clone()
            };
            stmt.execute(params![
                key,
                record.datetime,
                record.available_at,
                serde_json::to_string(&record.payload).map_err(|e| e.to_string())?
            ])
            .map_err(|e| e.to_string())?;
            count += 1;
        }
    }
    if count == 0 {
        return Err("匯入檔沒有資料".into());
    }
    let hash = format!("{:x}", reader.into_inner().hash.finalize());
    let effective: i64 = tx
        .query_row("SELECT COUNT(*) FROM incoming_base_bars", [], |r| r.get(0))
        .map_err(|e| e.to_string())?;
    let has_derived:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='qm_derived')",[],|r|r.get(0)).map_err(|e|e.to_string())?;
    if has_derived {tx.execute("DELETE FROM qm_derived WHERE dataset_id=?1 AND time_key IN(SELECT time_key FROM incoming_base_bars)",[id]).map_err(|e|e.to_string())?;}
    let replaced:i64=tx.query_row("SELECT COUNT(*) FROM market_bar_keys k JOIN incoming_base_bars i ON i.time_key=k.time_key WHERE k.dataset_id=?1",[id],|r|r.get(0)).map_err(|e|e.to_string())?;
    let base: i64 = tx
        .query_row(
            "SELECT COALESCE(MAX(ordinal),0) FROM records WHERE dataset_id=?1",
            [id],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    tx.execute("DELETE FROM records WHERE dataset_id=?1 AND ordinal IN(SELECT k.ordinal FROM market_bar_keys k JOIN incoming_base_bars i ON i.time_key=k.time_key WHERE k.dataset_id=?1)",[id]).map_err(|e|e.to_string())?;
    tx.execute("DELETE FROM market_bar_keys WHERE dataset_id=?1 AND time_key IN(SELECT time_key FROM incoming_base_bars)",[id]).map_err(|e|e.to_string())?;
    tx.execute("INSERT INTO records SELECT ?1,?2+ROW_NUMBER() OVER(ORDER BY time_key),datetime,available_at,payload FROM incoming_base_bars",params![id,base]).map_err(|e|e.to_string())?;
    tx.execute("INSERT INTO market_bar_keys SELECT ?1,time_key,?2+ROW_NUMBER() OVER(ORDER BY time_key) FROM incoming_base_bars",params![id,base]).map_err(|e|e.to_string())?;
    let (total, first, last): (i64, String, String) = tx
        .query_row(
            "SELECT COUNT(*),MIN(datetime),MAX(datetime) FROM records WHERE dataset_id=?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .map_err(|e| e.to_string())?;
    let lineage = format!(
        "{:x}",
        Sha256::digest(format!("base-upsert-v1:{oldhash}:{hash}").as_bytes())
    );
    tx.execute("UPDATE datasets SET sha256=?1,row_count=?2,first_datetime=?3,last_datetime=?4,imported_at=CURRENT_TIMESTAMP WHERE id=?5",params![lineage,total,first,last,id]).map_err(|e|e.to_string())?;
    tx.execute("INSERT INTO market_import_events(dataset_id,source_path,source_sha256,input_rows,effective_rows,replaced_rows) VALUES(?1,?2,?3,?4,?5,?6)",params![id,file.to_string_lossy(),hash,count,effective,replaced]).map_err(|e|e.to_string())?;
    tx.commit().map_err(|e| e.to_string())?;
    Ok(
        json!({"symbol":symbol,"kind":kind,"dataset_id":id,"input_rows":count,"effective_rows":effective,"replaced_rows":replaced,"total_rows":total,"first":first,"last":last,"source_sha256":hash}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    fn folder(label: &str) -> std::path::PathBuf {
        let p = crate::workbench::root().join("outputs").join(format!(
            "canonical-{label}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&p).unwrap();
        p
    }
    #[test]
    fn bars_last_row_wins_and_bad_import_rolls_back() {
        let dir = folder("bars");
        let db = dir.join("test.sqlite");
        let input = dir.join("minute.csv");
        std::fs::write(&input,"datetime,open,high,low,close,volume\n2026-10-01T09:01:00,100,103,99,101,2\n2026-10-01T09:01:00,100,104,99,103,3\n2026-10-01T09:02:00,103,106,102,105,4\n").unwrap();
        let a = import(&db, &input, "test", "minute", "unspecified", "end").unwrap();
        assert_eq!(a["input_rows"], 3);
        assert_eq!(a["total_rows"], 2);
        let id = a["dataset_id"].as_i64().unwrap();
        std::fs::write(&input,"datetime,open,high,low,close,volume\n2026-10-01T09:01:00,100,110,90,108,7\n2026-10-01T09:03:00,108,112,107,111,8\n").unwrap();
        let b = import(&db, &input, "TEST", "minute", "unspecified", "end").unwrap();
        assert_eq!(b["dataset_id"], id);
        assert_eq!(b["replaced_rows"], 1);
        assert_eq!(b["total_rows"], 3);
        let c = database::open_existing(&db).unwrap();
        let before = database::query(&c, id, "", "z", 10).unwrap();
        assert_eq!(before.len(), 3);
        assert!(matches!(&before[0].payload,database::Payload::Bar{close,..} if close=="108"));
        drop(c);
        std::fs::write(&input,"datetime,open,high,low,close,volume\n2026-10-01T09:01:00,100,110,90,109,9\n2026-10-01T09:04:00,100,50,90,99,1\n").unwrap();
        assert!(import(&db, &input, "TEST", "minute", "unspecified", "end").is_err());
        let c = database::open_existing(&db).unwrap();
        assert_eq!(
            serde_json::to_string(&database::query(&c, id, "", "z", 10).unwrap()).unwrap(),
            serde_json::to_string(&before).unwrap()
        );
        assert_eq!(sources(&db).unwrap().as_array().unwrap().len(), 1);
    }
    #[test]
    fn tick_range_replacement_removes_missing_middle_keeps_new_duplicates() {
        let dir = folder("tick");
        let db = dir.join("test.sqlite");
        let input = dir.join("tick.csv");
        std::fs::write(&input,"datetime,price,volume\n2026-10-01T09:00:00,100,1\n2026-10-01T09:00:01,101,1\n2026-10-01T09:00:02,102,1\n2026-10-01T09:00:02,102,1\n2026-10-01T09:00:03,103,1\n2026-10-01T09:00:05,105,1\n").unwrap();
        let a = import(&db, &input, "TEST", "tick", "unspecified", "event").unwrap();
        let id = a["dataset_id"].as_i64().unwrap();
        std::fs::write(&input,"datetime,price,volume\n2026-10-01T09:00:01,201,2\n2026-10-01T09:00:03,203,3\n2026-10-01T09:00:03,203,3\n").unwrap();
        let b = import(&db, &input, "TEST", "tick", "unspecified", "event").unwrap();
        assert_eq!(b["dataset_id"], id);
        assert_eq!(b["total_rows"], 5);
        let c = database::open_existing(&db).unwrap();
        let rows = database::query(&c, id, "", "z", 100).unwrap();
        assert!(!rows.iter().any(|r| r.datetime.contains("09:00:02")));
        assert_eq!(rows[2].datetime, rows[3].datetime);
        assert_eq!(tick_store::verify(&c, id).unwrap().count, 5);
        drop(c);
        let b = import(&db, &input, "TEST", "tick", "unspecified", "event").unwrap();
        assert_eq!(b["total_rows"], 5);
        std::fs::write(
            &input,
            "datetime,price,volume\n2026-10-01T09:00:01,999,1\n2026-10-01T09:00:03,999,-1\n",
        )
        .unwrap();
        assert!(import(&db, &input, "TEST", "tick", "unspecified", "event").is_err());
        let c = database::open_existing(&db).unwrap();
        assert_eq!(
            serde_json::to_string(&database::query(&c, id, "", "z", 100).unwrap()).unwrap(),
            serde_json::to_string(&rows).unwrap()
        );
    }
    #[test]
    fn tick_replacement_spans_blocks_and_shifts_tail_without_reordering() {
        use std::io::Write;
        let dir = folder("blocks");
        let db = dir.join("test.sqlite");
        let input = dir.join("tick.csv");
        {
            let mut file = std::io::BufWriter::new(File::create(&input).unwrap());
            writeln!(file, "datetime,price,volume").unwrap();
            for i in 0..200005 {
                writeln!(
                    file,
                    "2026-10-01T09:00:{:02},100,1",
                    if i < 100002 { 0 } else { 1 }
                )
                .unwrap();
            }
        }
        let a = import(&db, &input, "TEST", "tick", "unspecified", "event").unwrap();
        let id = a["dataset_id"].as_i64().unwrap();
        std::fs::write(
            &input,
            "datetime,price,volume\n2026-10-01T09:00:00,200,2\n2026-10-01T09:00:00,200,2\n",
        )
        .unwrap();
        let b = import(&db, &input, "TEST", "tick", "unspecified", "event").unwrap();
        assert_eq!(b["total_rows"], 100005);
        let c = database::open_existing(&db).unwrap();
        assert_eq!(tick_store::verify(&c, id).unwrap().count, 100005);
        assert_eq!(database::query(&c, id, "", "z", 3).unwrap().len(), 3);
    }
    #[test]
    fn source_routes_and_second_minute_daily_aggregation() {
        let dir = folder("routes");
        let db = dir.join("test.sqlite");
        let input = dir.join("bars.csv");
        let mut cfg: crate::backtest::Config = serde_json::from_value(
            crate::workbench::read_json(
                &crate::workbench::root().join("tasks/pl-workbench-example.json"),
            )
            .unwrap()["parameters"]
                .clone(),
        )
        .unwrap();
        cfg.from = "".into();
        cfg.to = "".into();
        cfg.warmup_bars = 0;
        cfg.sessions = vec![];
        cfg.price_scale = Some(1.0);
        cfg.minimum_move = Some(1);
        std::fs::write(&input,"datetime,open,high,low,close,volume\n2026-10-01T09:01:00,100,103,99,102,2\n2026-10-01T09:02:00,102,110,101,108,3\n2026-10-01T09:03:00,108,112,107,111,4\n").unwrap();
        import(&db, &input, "TEST", "minute", "unspecified", "end").unwrap();
        cfg.dataset_id = resolve(&db, "TEST", "minute").unwrap();
        cfg.interval_minutes = 2;
        cfg.source_interval_minutes = Some(1);
        let (bars, _) = crate::backtest::load(&db, &cfg).unwrap();
        assert_eq!(bars.len(), 2);
        assert_eq!(
            (
                bars[0].open,
                bars[0].high,
                bars[0].low,
                bars[0].close,
                bars[0].volume
            ),
            (100., 110., 99., 108., 5.)
        );
        std::fs::write(&input,"datetime,price,volume\n2026-10-01T09:00:00,100,1\n2026-10-01T09:00:29,103,2\n2026-10-01T09:00:30,102,3\n2026-10-01T09:00:59,105,4\n").unwrap();
        import(&db, &input, "TEST", "tick", "unspecified", "event").unwrap();
        cfg.dataset_id = resolve(&db, "TEST", "tick").unwrap();
        cfg.interval_minutes = 0;
        cfg.source_interval_minutes = None;
        cfg.interval_seconds = Some(30);
        let (bars, _) = crate::backtest::load(&db, &cfg).unwrap();
        assert_eq!(bars.len(), 2);
        assert_eq!(
            (bars[0].close, bars[0].volume, bars[1].open, bars[1].close),
            (103., 3., 102., 105.)
        );
        std::fs::write(&input,"datetime,open,high,low,close,volume\n2026-10-01,100,103,99,102,2\n2026-10-02,102,110,101,108,3\n2026-10-05,108,112,107,111,4\n").unwrap();
        import(&db, &input, "TEST", "daily", "unspecified", "end").unwrap();
        cfg.dataset_id = resolve(&db, "TEST", "daily").unwrap();
        cfg.interval_seconds = None;
        cfg.interval_days = Some(2);
        let (bars, _) = crate::backtest::load(&db, &cfg).unwrap();
        assert_eq!(bars.len(), 2);
        assert!(bars[0].datetime.starts_with("2026-10-02"));
        assert_eq!(bars[0].volume, 5.);
        assert_eq!(sources(&db).unwrap().as_array().unwrap().len(), 3);
        assert!(resolve(&db, "MISSING", "tick").is_err());
    }
}
