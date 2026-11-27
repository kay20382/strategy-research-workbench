//! Reaggregate canonical ticks while excluding exact session-close timestamps.
use recharts_core::{database,tick_bars::{Aggregator,TickBar}};
use sha2::{Digest,Sha256};
use chrono::Timelike;
use std::{path::Path,fs};
fn run()->Result<(),String>{
 let args:Vec<_>=std::env::args().collect();
 if args.len()!=4{return Err("用法：test_close_boundary 資料庫 Tick資料集ID 新輸出目錄".into());}
 let out=Path::new(&args[3]);if out.exists(){return Err("輸出目錄已存在".into());}
 let id:i64=args[2].parse().map_err(|_|"資料集ID錯誤")?;
 let c=database::open_existing(Path::new(&args[1]))?;
 let mut stmt=c.prepare("SELECT block_no,data,sha256,tick_count FROM tick_blocks WHERE dataset_id=? ORDER BY block_no").map_err(|e|e.to_string())?;
 let mut agg=Aggregator::with_sessions(900)?;
 fs::create_dir_all(out).map_err(|e|e.to_string())?;
 let mut w=csv::Writer::from_path(out.join("bars.csv")).map_err(|e|e.to_string())?;
 let mut total=0u64;let mut removed=0u64;let mut removed_volume=0i128;let mut count=0u64;let mut previous=None;let mut blocks=0i64;
 let mut write=|bar:TickBar|->Result<(),String>{w.serialize(bar).map_err(|e|e.to_string())?;count+=1;Ok(())};
 let rows=stmt.query_map([id],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,Vec<u8>>(1)?,r.get::<_,String>(2)?,r.get::<_,i64>(3)?))).map_err(|e|e.to_string())?;
 for row in rows {
  let (no,data,hash,n)=row.map_err(|e|e.to_string())?;
  if no!=blocks || n<=0 || data.len()!=n as usize*24 || format!("{:x}",Sha256::digest(&data))!=hash{return Err("Tick區塊驗證失敗".into());}blocks+=1;
  for t in data.chunks_exact(24){
   let num=|b:&[u8]|i64::from_le_bytes(b.try_into().unwrap());let ns=num(&t[..8]);let price=num(&t[8..16]);let volume=num(&t[16..24]);
   if previous.is_some_and(|p|ns<p){return Err("Tick時間倒退".into());}previous=Some(ns);total+=1;
   let dt=chrono::DateTime::from_timestamp(ns.div_euclid(1_000_000_000),ns.rem_euclid(1_000_000_000) as u32).ok_or("時間無效")?.naive_utc();
   if dt.nanosecond()==0 && (dt.num_seconds_from_midnight()==49500 || dt.num_seconds_from_midnight()==18000){removed+=1;removed_volume+=volume as i128;continue;}
   let p=format!("{:.6}",price as f64/1_000_000.);let v=format!("{:.6}",volume as f64/1_000_000.);
   if let Some(bar)=agg.push(dt,&p,&v)?{write(bar)?;}
  }
  if blocks%200==0{eprintln!("已處理{blocks}個Tick區塊");}
 }
 if let Some(bar)=agg.finish()?{write(bar)?;}drop(write);w.flush().map_err(|e|e.to_string())?;
 let result=serde_json::json!({"dataset_id":id,"total_ticks":total,"excluded_ticks":removed,"excluded_volume_fixed6":removed_volume.to_string(),"bars":count,"verified_blocks":blocks,"rule":"僅排除恰好13:45:00.000000000及05:00:00.000000000"});
 fs::write(out.join("aggregation.json"),serde_json::to_vec_pretty(&result).unwrap()).map_err(|e|e.to_string())?;println!("{result}");Ok(())
}
fn main(){if let Err(e)=run(){eprintln!("{e}");std::process::exit(1)}}
