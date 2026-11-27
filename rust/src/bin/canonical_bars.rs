//! Build a derived view directly from canonical tick_blocks, without copying ticks.
use recharts_core::{database, tick_bars::Aggregator};
use chrono::{DateTime, Timelike};
use sha2::{Digest, Sha256};
use std::{fs, path::Path};
fn run() -> Result<(), String> {
    let args: Vec<_> = std::env::args().collect();
    if args.len() != 5 { return Err("用法：canonical_bars 資料庫 Tick資料集ID 新輸出目錄 週期秒數".into()); }
    let id: i64 = args[2].parse().map_err(|_| "資料集ID須為整數")?;
    let seconds: i64 = args[4].parse().map_err(|_| "週期須為整數秒")?;
    let output=Path::new(&args[3]);
    if output.exists() { return Err("輸出目錄已存在，請使用新路徑".into()); }
    let c=database::open_existing(Path::new(&args[1]))?;
    if !recharts_core::tick_store::contains(&c,id)? { return Err("來源不是 canonical tick_blocks".into()); }
    let (source_sha, expected): (String,i64)=c.query_row("SELECT sha256,row_count FROM datasets WHERE id=?",[id],|r| Ok((r.get(0)?,r.get(1)?))).map_err(|e|e.to_string())?;
    fs::create_dir_all(output).map_err(|e|e.to_string())?;
    let part=output.join("bars.csv.partial");
    let mut writer=csv::Writer::from_path(&part).map_err(|e|e.to_string())?;
    let mut aggregator=Aggregator::with_sessions_close_exclusive(seconds)?;
    let mut stmt=c.prepare("SELECT block_no,tick_count,data,sha256 FROM tick_blocks WHERE dataset_id=? ORDER BY block_no").map_err(|e|e.to_string())?;
    let mut rows=stmt.query([id]).map_err(|e|e.to_string())?;
    let mut blocks=0; let mut ticks=0u64;let mut bars=0u64;let mut previous=None;
    let mut close_count=0u64;let mut close_volume=0i128;
    let mut volume=0i128;let mut block_hasher=Sha256::new();
    while let Some(row)=rows.next().map_err(|e|e.to_string())? {
        let no:i64=row.get(0).map_err(|e|e.to_string())?;
        let count:i64=row.get(1).map_err(|e|e.to_string())?;
        let data:Vec<u8>=row.get(2).map_err(|e|e.to_string())?;
        let hash:String=row.get(3).map_err(|e|e.to_string())?;
        if no != blocks || count<=0 || data.len() != count as usize*24 || format!("{:x}",Sha256::digest(&data)) != hash { return Err("Tick 區塊順序、長度或雜湊錯誤".into()); }
        block_hasher.update(hash.as_bytes());
        for raw in data.chunks_exact(24) {
            let number=|v:&[u8]| i64::from_le_bytes(v.try_into().unwrap());
            let ns=number(&raw[..8]);let p=number(&raw[8..16]);let v=number(&raw[16..24]);
            if previous.is_some_and(|last| ns<last) || v<0 { return Err("Tick 時間倒退或負成交量".into()); }
            previous=Some(ns);ticks+=1;volume+=v as i128;
            let dt=DateTime::from_timestamp(ns.div_euclid(1_000_000_000),ns.rem_euclid(1_000_000_000) as u32).ok_or("Tick時間超出範圍")?.naive_utc();
            if [18000,49500].contains(&dt.num_seconds_from_midnight()) {close_count+=1;close_volume+=v as i128;continue;}
            if let Some(bar)=aggregator.push_fixed(dt,p as i128,v as i128)? { writer.serialize(bar).map_err(|e|e.to_string())?;bars+=1; }
        }
        blocks+=1;
        if blocks%200==0 { eprintln!("已核對 {blocks} 區塊／{ticks} 筆 Tick"); }
    }
    if ticks != expected as u64 { return Err("來源 Tick 總筆數不符".into()); }
    if let Some(bar)=aggregator.finish()? {writer.serialize(bar).map_err(|e|e.to_string())?;bars+=1;}
    writer.flush().map_err(|e|e.to_string())?; drop(writer);
    let target=output.join("bars.csv");fs::rename(part,&target).map_err(|e|e.to_string())?;
    let csv_sha=format!("{:x}",Sha256::digest(fs::read(&target).map_err(|e|e.to_string())?));
    let summary=serde_json::json!({"schema":"canonical-session-bars-v1","source_dataset_id":id,"source_sha256":source_sha,"source_ticks":ticks,"source_blocks":blocks,"block_hashes_sha256":format!("{:x}",block_hasher.finalize()),"bars":bars,"seconds":seconds,"timestamp_role":"end","sessions":"日盤08:45至13:45；夜盤15:00至05:00；[start,end)","close_excluded_ticks":close_count,"close_excluded_volume":close_volume as f64/1_000_000.0,"source_volume":volume as f64/1_000_000.0,"bars_sha256":csv_sha,"mc_bar_equivalence_verified":false,"gaps_filled":false,"deduplicated":false});
    fs::write(output.join("aggregation.json"),serde_json::to_vec_pretty(&summary).map_err(|e|e.to_string())?).map_err(|e|e.to_string())?;
    println!("{summary}");Ok(())
}
fn main(){if let Err(e)=run(){eprintln!("彙整失敗：{e}");std::process::exit(1)}}
