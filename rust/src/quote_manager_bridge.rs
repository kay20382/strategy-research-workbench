//! Godot 僅收送 JSON；背景工作不存取任何 Godot 物件。
use godot::prelude::*;
use serde_json::{json, Value};
use std::{path::Path, sync::mpsc};

#[derive(GodotClass)]
#[class(base=RefCounted, init)]
struct QuoteManagerBridge {
    base: Base<RefCounted>,
    pending: Option<mpsc::Receiver<String>>,
}

fn execute(v: Value) -> Result<Value, String> {
    let s = |key: &str| v[key].as_str().ok_or_else(|| format!("缺少參數：{key}"));
    let db = Path::new(s("database")?);
    let root = Path::new(s("root")?);
    crate::quote_manager::initialize(root, db)?;
    let qm = &v["instrument"];
    match s("action")? {
        "inventory" => crate::quote_manager::inventory(db),
        "inspect_import" => crate::market_csv::inspect(Path::new(s("file")?)),
        "delete_plan" => crate::quote_manager::delete_plan(db,s("symbol")?),
        "delete" => crate::quote_manager::delete_instrument(db,s("symbol")?,s("confirmation")?,s("token")?,v["delete_data"].as_bool().unwrap_or(false)),
        "defaults" => Ok(json!(crate::quote_manager::defaults(s("symbol")?))),
        "save" => {
            let instrument = serde_json::from_value(qm.clone()).map_err(|e|e.to_string())?;
            crate::quote_manager::save(db, &instrument)?;
            Ok(json!({"saved":true}))
        }
        "coverage" => crate::quote_manager::coverage(db,s("symbol")?,s("from")?,s("to")?),
        "coverage_all" => crate::quote_manager::coverage_all(db,s("symbol")?),
        "fill" => crate::quote_manager::fill_from_ticks(db,s("symbol")?,s("from")?,s("to")?),
        "fill_all" => crate::quote_manager::fill_all(db,s("symbol")?),
        "import" => crate::quote_manager::import(db,Path::new(s("file")?),s("symbol")?,s("kind")?,s("timezone")?,s("role")?),
        "export" => crate::quote_manager::export(db,s("symbol")?,s("kind")?,s("from")?,s("to")?,Path::new(s("file")?)),
        "annotate" => crate::quote_manager::annotate_source(db,s("symbol")?,s("kind")?,s("timezone")?,s("role")?),
        _ => Err("未知的資料管理操作".into()),
    }
}

#[godot_api]
impl QuoteManagerBridge {
    #[func]
    fn start(&mut self, request: GString) -> bool {
        if self.pending.is_some() { return false; }
        let request = request.to_string();
        let (tx,rx) = mpsc::channel();
        self.pending = Some(rx);
        std::thread::spawn(move || {
            let result = std::panic::catch_unwind(|| {
                serde_json::from_str(&request).map_err(|e|e.to_string()).and_then(execute)
            }).unwrap_or_else(|_| Err("背景工作非預期中止，請重新整理並檢查操作結果".into()));
            let response = match result {
                Ok(data) => json!({"ok":true,"data":data}),
                Err(error) => json!({"ok":false,"error":error}),
            };
            let _ = tx.send(response.to_string());
        });
        true
    }

    #[func]
    fn poll(&mut self) -> GString {
        let Some(rx) = &self.pending else { return GString::new(); };
        match rx.try_recv() {
            Ok(value) => { self.pending = None; GString::from(&value) },
            Err(mpsc::TryRecvError::Empty) => GString::new(),
            Err(_) => {
                self.pending = None;
                GString::from("{\"ok\":false,\"error\":\"背景工作已中斷，請重新整理\"}")
            }
        }
    }
}
