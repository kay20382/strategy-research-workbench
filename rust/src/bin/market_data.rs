use recharts_core::database::{self, DataKind};
use std::path::Path;
fn run()->Result<(),String> {
    let a:Vec<String>=std::env::args().collect();
    if a.len()==3 && a[1]=="sources" {
        recharts_core::canonical_market::initialize(&recharts_core::workbench::root(),Path::new(&a[2]))?;
        println!("{}",recharts_core::canonical_market::sources(Path::new(&a[2]))?);
    } else if a.len()==9 && a[1]=="upsert" {
        if a[8]!="v1"{return Err("介面版本須為v1".into());}
        let r=recharts_core::canonical_market::import(Path::new(&a[2]),Path::new(&a[3]),&a[5],&a[4],&a[6],&a[7])?;
        println!("{r}");
    } else if a.len()==10 && ["import","import-legacy"].contains(&a[1].as_str()) {
        if a[9]!="v1" { return Err("介面版本須為 v1".into()); }
        let kind=DataKind::parse(&a[4])?;
        if a[1]=="import" && [DataKind::Minute,DataKind::Daily].contains(&kind){
            recharts_core::canonical_market::initialize(&recharts_core::workbench::root(),Path::new(&a[2]))?;
            let summary=recharts_core::canonical_market::import(Path::new(&a[2]),Path::new(&a[3]),&a[5],kind.name(),&a[7],&a[8])?;println!("{summary}");
        }else{
            let mut c=database::open(Path::new(&a[2]))?;
            let summary=database::import_file(&mut c,Path::new(&a[3]),kind,&a[5],&a[6],&a[7],&a[8])?;
            println!("{}",serde_json::to_string_pretty(&summary).map_err(|e|e.to_string())?);
        }
    } else if a.len()==7 && a[1]=="query" {
        let c=database::open_existing(Path::new(&a[2]))?;
        let id=a[3].parse().map_err(|_|"資料集識別碼須為整數")?;
        let from=if a[4]=="*" {String::new()}else{a[4].clone()};let to=if a[5]=="*" {"z".into()}else{a[5].clone()};
        let rows=database::query(&c,id,&from,&to,a[6].parse().map_err(|_|"筆數須為整數")?)?;
        println!("{}",serde_json::to_string_pretty(&rows).map_err(|e|e.to_string())?);
    } else { return Err("用法：market_data import 資料庫 檔案 trade|quote|minute|daily|custom 商品 來源 時區 start|end|event|unspecified v1\n或：market_data query 資料庫 資料集ID 起點或* 終點或* 筆數".into()); }
    Ok(())
}
fn main(){if let Err(e)=run(){eprintln!("{e}");std::process::exit(1);}}
