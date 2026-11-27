use recharts_core::{quote_manager as qm, workbench};
use std::{fs, path::Path};
#[cfg(windows)]
#[path = "quote_manager/ui.rs"]
mod ui;
fn run() -> Result<(), String> {
    let a: Vec<String> = std::env::args().collect();
    if a.len() == 1 || (a.len() == 2 && a[1].starts_with("--database=")) {
        #[cfg(windows)]
        unsafe {
            ui::run();
            return Ok(());
        }
        #[cfg(not(windows))]
        return Err("UI僅適用Windows，CLI可跨平台使用".into());
    }
    let cmd = a.get(1).map(String::as_str).unwrap_or("");
    if a.len() < 3 {
        return Err("用法：quote_manager inventory 資料庫 | save 資料庫 商品.json | coverage 資料庫 商品 [起日 迄日]（省略則檢查三來源最寬期間，不限天數） | fill 資料庫 商品 起日 迄日 | export 資料庫 商品 tick|minute|daily 起日 迄日 新檔 | import 資料庫 商品 格式 時區 role 檔案".into());
    }
    let db = Path::new(&a[2]);
    qm::initialize(&workbench::root(), db)?;
    let value = match (cmd, a.len()) {
        ("inventory", 3) => qm::inventory(db)?,
        ("save", 4) => {
            let i = serde_json::from_str(&fs::read_to_string(&a[3]).map_err(|e| e.to_string())?)
                .map_err(|e| e.to_string())?;
            qm::save(db, &i)?;
            serde_json::json!({"saved":true})
        }
        ("coverage", 4) => qm::coverage_all(db, &a[3])?,
        ("coverage", 6) => qm::coverage(db, &a[3], &a[4], &a[5])?,
        ("fill", 6) => qm::fill_from_ticks(db, &a[3], &a[4], &a[5])?,
        ("export", 8) => qm::export(db, &a[3], &a[4], &a[5], &a[6], Path::new(&a[7]))?,
        ("import", 8) => qm::import(db, Path::new(&a[7]), &a[3], &a[4], &a[5], &a[6])?,
        _ => return Err("命令或參數數量錯誤".into()),
    };
    println!("{}", serde_json::to_string_pretty(&value).unwrap());
    Ok(())
}
fn main() {
    if let Err(e) = run() {
        eprintln!("{e}");
        std::process::exit(1);
    }
}
