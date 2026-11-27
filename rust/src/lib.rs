pub mod api_connection;
mod market;
pub mod database;
pub mod date_format;
mod market_csv;
pub mod powerlanguage;
pub mod pl_exchange;
pub mod pl_diff;
pub mod backtest;
pub mod robustness;
pub mod workbench;
mod workbench_report;
pub mod canonical_market;
pub mod quote_manager;
pub mod report_statistics;
mod quote_manager_bridge;
mod workbench_bridge;
pub mod strategy_library;
pub mod strategy_versions;
pub mod strategy_logic;
pub mod strategy_research;
pub mod strategy_requirements;
pub mod tick_bars;
pub mod tick_store;
pub mod tick_execution;
pub mod live_market;
mod live_chart;
mod live_strategy;
mod foreign_strategy_replay;
pub mod api_profiles;
pub mod trading;

use godot::prelude::*;
use market::{aggregate, demo, enrich, parse_csv, Bar};

struct ReChartsExtension;
#[gdextension]
unsafe impl ExtensionLibrary for ReChartsExtension {}

#[derive(GodotClass)]
#[class(base=RefCounted, init)]
struct ChartEngine {
    base: Base<RefCounted>,
}

fn response(result: Result<Vec<Bar>, String>) -> GString {
    let value = match result {
        Ok(bars) => serde_json::json!({"bars": enrich(&bars), "error": ""}),
        Err(error) => serde_json::json!({"bars": [], "error": error}),
    };
    GString::from(value.to_string().as_str())
}

#[godot_api]
impl ChartEngine {
    #[func]
    fn import_dataset_json(&self, database_path: GString, file_path: GString, options_json: GString) -> GString {
        #[derive(serde::Deserialize)]
        struct Options { kind: database::DataKind, symbol: String, source: String, timezone: String, timestamp_role: String }
        let result = (|| -> Result<database::ImportSummary,String> {
            let o:Options=serde_json::from_str(&options_json.to_string()).map_err(|e|e.to_string())?;
            let mut conn=database::open(std::path::Path::new(&database_path.to_string()))?;
            database::import_file(&mut conn,std::path::Path::new(&file_path.to_string()),o.kind,&o.symbol,&o.source,&o.timezone,&o.timestamp_role)
        })();
        let value=match result {Ok(summary)=>serde_json::json!({"dataset":summary,"error":""}),Err(error)=>serde_json::json!({"dataset":null,"error":error})};
        GString::from(value.to_string().as_str())
    }

    #[func]
    fn query_dataset_json(&self, database_path: GString, dataset_id: i64, query_json: GString) -> GString {
        #[derive(serde::Deserialize)]
        struct Query { from: String, to: String, limit: usize }
        let result=(|| -> Result<Vec<database::Record>,String> {
            let q:Query=serde_json::from_str(&query_json.to_string()).map_err(|e|e.to_string())?;
            let conn=database::open_existing(std::path::Path::new(&database_path.to_string()))?;
            database::query(&conn,dataset_id,&q.from,&q.to,q.limit)
        })();
        let value=match result {Ok(records)=>serde_json::json!({"records":records,"error":""}),Err(error)=>serde_json::json!({"records":[],"error":error})};
        GString::from(value.to_string().as_str())
    }

    #[func]
    fn demo_json(&self, seed: i64) -> GString {
        response(Ok(demo(seed as u64)))
    }

    #[func]
    fn csv_json(&self, text: GString) -> GString {
        response(parse_csv(&text.to_string()))
    }

    #[func]
    fn aggregate_json(&self, text: GString, period: i64) -> GString {
        let result = serde_json::from_str::<Vec<Bar>>(&text.to_string())
            .map_err(|e| format!("行情資料格式錯誤：{e}"))
            .and_then(|bars| aggregate(&bars, period));
        response(result)
    }
}

