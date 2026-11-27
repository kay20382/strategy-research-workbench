use recharts_core::{database, tick_store};
use std::{fs, path::Path};
fn run() -> Result<(), String> {
    let a = std::env::args().collect::<Vec<_>>();
    if (a.len() == 6 || a.len() == 7) && a[1] == "import" {
        let out = Path::new(&a[5]);
        if out.exists() {
            return Err("報告已存在，請使用新路徑".into());
        }
        let r = tick_store::import_format(
            Path::new(&a[2]),
            Path::new(&a[3]),
            &a[4],
            a.get(6).map_or("mc_csv", String::as_str),
        )?;
        if let Some(p) = out.parent() {
            fs::create_dir_all(p).map_err(|e| e.to_string())?
        }
        fs::write(
            out,
            serde_json::to_vec_pretty(&r).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
        println!(
            "{}",
            serde_json::to_string_pretty(&r).map_err(|e| e.to_string())?
        );
        Ok(())
    } else if a.len() == 5 && a[1] == "verify" {
        let out = Path::new(&a[4]);
        if out.exists() {
            return Err("驗證報告已存在".into());
        }
        let c = database::open_existing(Path::new(&a[2]))?;
        let r = tick_store::verify(&c, a[3].parse().map_err(|_| "資料集ID錯誤")?)?;
        fs::write(
            out,
            serde_json::to_vec_pretty(&r).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
        println!(
            "{}",
            serde_json::to_string_pretty(&r).map_err(|e| e.to_string())?
        );
        Ok(())
    } else {
        Err("用法：tick_store import 資料庫 Tick檔 商品 新報告.json；或 tick_store verify 資料庫 資料集ID 新驗證.json".into())
    }
}
fn main() {
    if let Err(e) = run() {
        eprintln!("{e}");
        std::process::exit(1)
    }
}
