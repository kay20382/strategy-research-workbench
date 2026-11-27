use chrono::{NaiveDate, NaiveDateTime};
use recharts_core::{
    canonical_market, database, strategy_library as library, strategy_research as research,
    workbench,
};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    fs,
    io::{self, BufRead},
    path::{Path, PathBuf},
};

#[path = "strategy_mcp/research_loop.rs"]
mod research_loop;
#[path = "strategy_mcp/performance.rs"]
mod performance;
#[path = "strategy_mcp/jobs.rs"]
mod jobs;

const PROTOCOLS: [&str; 3] = ["2025-11-25", "2025-06-18", "2025-03-26"];
use recharts_core::strategy_requirements::{self, INSTRUCTIONS as RESEARCH_ACCEPTANCE};

fn tool(
    name: &str,
    description: &str,
    properties: Value,
    required: &[&str],
    read_only: bool,
) -> Value {
    json!({
        "name": name, "description": description,
        "inputSchema": {"type":"object","properties":properties,"required":required,"additionalProperties":false},
        "annotations": {"readOnlyHint":read_only,"destructiveHint":false,"openWorldHint":false}
    })
}

fn tools() -> Value {
    let strategy = json!({"type":"string","description":"strategy_list 傳回的 id"});
    let source = json!({"type":"string","description":"完整 PowerLanguage 原始碼"});
    let run = json!({"type":"string","description":"回測傳回的 run_id"});
    let code_ref = json!({"type":"object","description":"三欄擇一","additionalProperties":false,
        "properties":{"strategy_id":strategy,"run_id":run,"source":source}});
    let mut tools = vec![
        tool("api_profile_schema", "取得指定券商與用途的欄位定義。凱基 QuoteCom 行情有六個指定參數，自訂券商支援公開與敏感參數。", json!({
            "provider":{"type":"string"},"purpose":{"type":"string","enum":["quote","trade"]}
        }), &["provider","purpose"], true),
        tool("api_profile_list", "列出 API 連線設定與憑證設定狀態。", json!({}), &[], true),
        tool(
            "api_profile_save",
            "建立或更新 API 設定。同一供應商及用途若省略憑證欄位，沿用原值；空字串清除。正式憑證可從工作台輸入，避免出現在工具呼叫紀錄。",
            json!({
                "profile":{"type":"object","additionalProperties":false,"required":["id","provider","purpose"],"properties":{
                    "id":{"type":"string","pattern":"^[A-Za-z0-9_-]{1,64}$"},
                    "provider":{"type":"string","description":"例如 kgi_quotecom"},
                    "purpose":{"type":"string","enum":["quote","trade"]},
                    "connection":{"type":"string","description":"券商指定的連線字串前綴"},
                    "host":{"type":"string"},"port":{"type":["integer","null"],"minimum":1,"maximum":65535},
                    "source_id":{"type":"string"},"ca_path":{"type":"string","description":"憑證本機絕對路徑"},
                    "options":{"type":"object","additionalProperties":{"type":"string"},"description":"自訂公開參數"}
                }},
                "credentials":{"type":"object","additionalProperties":false,"properties":{
                    "account":{"type":"string","description":"凱基 QuoteCom UserID"},"password":{"type":"string"},"token":{"type":"string"},"ca_password":{"type":"string"},
                    "secrets":{"type":"object","additionalProperties":{"type":"string"},"description":"自訂敏感參數。省略保留，空字串清除。"}
                }}
            }),
            &["profile"],
            false,
        ),
        tool(
            "market_sources",
            "列出本機 Tick／1 分 K／日 K 來源、資料筆數及起訖時間。",
            json!({"symbol":{"type":"string"}}),
            &[],
            true
        ),
        tool(
            "market_query",
            "依商品及來源查詢實際行情，區間包含起訖。時間使用來源時區，回傳至多 1000 筆。Tick 可將 next_cursor 原樣帶回 cursor。",
            json!({"symbol":{"type":"string"},"kind":{"enum":["tick","minute","daily"]},
                "from":{"type":"string","description":"YYYY-MM-DD 或 YYYY-MM-DDTHH:MM:SS[.nnnnnnnnn]"},
                "to":{"type":"string","description":"YYYY-MM-DD 或 YYYY-MM-DDTHH:MM:SS[.nnnnnnnnn]"},
                "limit":{"type":"integer","minimum":1,"maximum":1000,"default":100},
                "cursor":{"type":"object","description":"僅限 Tick，請原樣帶回上頁 next_cursor，商品及起訖維持相同。",
                    "properties":{"dataset_id":{"type":"integer"},"datetime":{"type":"string"},"ordinal":{"type":"integer","minimum":1},
                        "from":{"type":"string"},"to":{"type":"string"},"revision":{"type":"string"}},
                    "required":["dataset_id","datetime","ordinal","from","to","revision"],"additionalProperties":false}}),
            &["symbol", "kind", "from", "to"],
            true
        ),
        tool(
            "strategy_list",
            "列出工作台策略庫及函式。",
            json!({}),
            &[],
            true
        ),
        tool(
            "strategy_read",
            "依策略庫 id 讀取 PL 原始碼與回測設定。",
            json!({"strategy_id":strategy}),
            &["strategy_id"],
            true
        ),
        tool(
            "strategy_diff",
            "逐行比較修改前後的 PowerLanguage，另匯出左右對照及醒目標示的 HTML。兩側均可選策略庫、回測快照或貼入原碼。",
            json!({"before":code_ref,"after":code_ref}), &["before","after"], false,
        ),
        tool(
            "strategy_export_pl",
            "輸出供繁體中文版 PowerLanguage 使用的 Big5／CP950 PL 原始碼，保留中文註釋。原碼若含不可編碼字元會回報錯誤。",
            code_ref["properties"].clone(), &[], false,
        ),
        tool(
            "strategy_version",
            "管理桌面工作台共用的策略 Git 版本。可列出、提交、讀取、從舊版建立草稿，或向指定 GitHub 分支擷取及推送。",
            json!({"strategy_id":strategy,"operation":{"type":"string","enum":["list","commit","read","draft","fetch","push"]},
                "message":{"type":"string"},"source":source,"inputs":{"type":"object","additionalProperties":{"type":"number"}},
                "parameters":{"type":"object","description":"回測參數欄位，可部分覆寫"},
                "version":{"type":"string"},"remote":{"type":"string"},"branch":{"type":"string"},"confirmed":{"type":"boolean"}}),
            &["strategy_id","operation"], false,
        ),
        tool(
            "strategy_check",
            "使用本機 PL 相容引擎檢查程式碼。可提供策略庫 id 以沿用相依函式。",
            json!({"strategy_id":strategy,"source":source}),
            &[],
            true
        ),
        tool(
            "strategy_requirements",
            "由 MCP 提供驗收題目與完整選項、建議答案和自行描述，供用戶端原樣呈現。",
            json!({"strategy_id":strategy,"request":{"type":"string","description":"使用者原始需求"},
                "mode":{"type":"string","enum":["new","existing"],"description":"從零製作策略時指定 new"},
                "draft_context":{"type":"object","description":"工作台尚未儲存草稿的回測設定，僅用於產生問題，不會寫入策略庫"},"acceptance":{"type":"object","additionalProperties":false,
                "properties":{"market":{"type":"string"},"trade_rules":{"type":"string"},
                    "backtest":{"type":"string"},"pass_criteria":{"type":"string"},"evidence":{"type":"string"},
                    "product":{"type":"string"},"bar_period":{"type":"string"},"market_session":{"type":"string"},
                    "strategy_goal":{"type":"string"},"entry_rules":{"type":"string"},"exit_rules":{"type":"string"},
                    "stop_loss":{"type":"string"},"take_profit":{"type":"string"}}}}),
            &[],
            true
        ),
        tool(
            "strategy_logic",
            "依相容引擎解析策略條件及委託，回傳靜態檢查、流程節點與路徑。可另存離線 HTML 報告與可編輯的 Excalidraw 流程圖。",
            json!({"strategy_id":strategy,"source":source,"export_html":{"type":"boolean"}}),
            &[], false,
        ),
        tool(
            "backtest_run",
            "沿用策略庫設定回測原版或指定的 PL。可覆寫日期。模糊的策略需求須先經 strategy_requirements 和使用者確認驗收。",
            json!({"strategy_id":strategy,"source":source,"from":{"type":"string"},"to":{"type":"string"},
                "initial_capital":{"type":"number","exclusiveMinimum":0},"warmup_bars":{"type":"integer","minimum":0}}),
            &["strategy_id"],
            false
        ),
        tool(
            "backtest_result",
            "讀取回測收據及所選頁面的逐筆交易。頁碼自 1 起算，每頁 100 筆。",
            json!({"run_id":run,"page":{"type":"integer","minimum":1,"default":1}}),
            &["run_id"],
            true
        ),
        tool(
            "robustness_run",
            "依已完成的 OHLC 基準，執行 K 棒雜訊、移動區塊拔靴及成交成本壓力測試。",
            json!({"run_id":{"type":"string"},"seed":{"type":"integer","minimum":0,"maximum":4294967295u32},
                "repetitions":{"type":"integer","minimum":1,"maximum":100},
                "noise_ticks":{"type":"integer","minimum":1,"maximum":10},
                "block_bars":{"type":"integer","minimum":2,"maximum":500}}),
            &["run_id"], false,
        ),
        tool(
            "optimization_compare",
            "回測外部 AI 提供的候選 PL，驗證行情與設定相同後回傳原版、候選版和績效差值。",
            json!({"baseline_run_id":run,"source":source}),
            &["baseline_run_id", "source"],
            false
        ),
        tool(
            "report_read",
            "依 run_id 查詢完整損益。可匯出 HTML 或 CSV；style 選 full（預設）或 simple（簡式）。",
            json!({"run_id":run,"include_source":{"type":"boolean","default":false},"format":{"type":"string","enum":["html","csv"]},"style":{"type":"string","enum":["full","simple"],"default":"full"},"date_basis":{"type":"string","enum":["auto","source_calendar"],"default":"auto"}}),
            &["run_id"],
            false
        ),
        tool(
            "model_update_report",
            "比較兩個已完成回測的更新前後版本，列出設定差異、績效差值與逐月比較。可選擇 HTML 或 CSV，不執行回測。",
            json!({"baseline_run_id":run,"candidate_run_id":run,"format":{"type":"string","enum":["html","csv"]},"date_basis":{"type":"string","enum":["auto","source_calendar"],"default":"auto"}}),
            &["baseline_run_id","candidate_run_id"],
            false
        ),
    ];
    tools.extend(research_loop::tools());
    tools.extend(jobs::tools());
    json!(tools)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MarketSource {
    symbol: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MarketQuery {
    symbol: String,
    kind: String,
    from: String,
    to: String,
    limit: Option<usize>,
    cursor: Option<TickCursor>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct TickCursor {
    dataset_id: i64,
    datetime: String,
    ordinal: i64,
    from: String,
    to: String,
    revision: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StrategyId {
    strategy_id: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StrategyCheck {
    strategy_id: Option<String>,
    source: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StrategyLogic {
    strategy_id: Option<String>,
    source: Option<String>,
    #[serde(default)]
    export_html: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Backtest {
    strategy_id: String,
    source: Option<String>,
    from: Option<String>,
    to: Option<String>,
    initial_capital: Option<f64>,
    warmup_bars: Option<usize>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BacktestResult {
    run_id: String,
    page: Option<usize>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Candidate {
    baseline_run_id: String,
    source: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Report {
    run_id: String,
    #[serde(default)]
    include_source: bool,
    format: Option<performance::Format>,
    #[serde(default)]
    style: performance::Style,
    #[serde(default)]
    date_basis: performance::DateBasis,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UpdateReport {
    baseline_run_id: String,
    candidate_run_id: String,
    format: Option<performance::Format>,
    #[serde(default)]
    date_basis: performance::DateBasis,
}

fn parse<T: DeserializeOwned>(value: Value) -> Result<T, String> {
    serde_json::from_value(value).map_err(|err| err.to_string())
}

fn timestamp(input: &str, end_of_day: bool) -> Result<String, String> {
    let text = input.trim().replace('/', "-").replace(' ', "T");
    let datetime = if text.len() == 10 {
        NaiveDate::parse_from_str(&text, "%Y-%m-%d")
            .ok()
            .and_then(|day| {
                if end_of_day {
                    day.and_hms_nano_opt(23, 59, 59, 999_999_999)
                } else {
                    day.and_hms_opt(0, 0, 0)
                }
            })
    } else {
        NaiveDateTime::parse_from_str(&text, "%Y-%m-%dT%H:%M:%S%.f").ok()
    };
    datetime
        .map(|value| value.format("%Y-%m-%dT%H:%M:%S%.9f").to_string())
        .ok_or_else(|| format!("時間格式錯誤：{input}"))
}

#[derive(Clone)]
struct Server {
    root: PathBuf,
    market: PathBuf,
    protocol: Option<&'static str>,
    ready: bool,
    job: Option<std::sync::Arc<jobs::Job>>,
}

fn valid_id(id: &str, prefix: &str) -> bool {
    id.strip_prefix(prefix)
        .and_then(|tail| tail.split_once('-'))
        .is_some_and(|(time, pid)| {
            time.parse::<u128>().is_ok()
                && pid.parse::<u32>().is_ok()
                && [time, pid]
                    .iter()
                    .all(|part| part.bytes().all(|c| c.is_ascii_digit()))
        })
}

impl Server {
    fn completed_report(&self, id: &str) -> Result<(PathBuf, Value, Value), String> {
        let dir = self.run_dir(id)?;
        let receipt = workbench::read_json(&dir.join("result.json"))?;
        if receipt["status"] != "completed" {
            return Err(format!("回測尚未完成：{}", receipt["error"].as_str().unwrap_or("沒有成功報表")));
        }
        let report = workbench::read_json(&dir.join("engine/report.json"))?;
        Ok((dir, receipt, report))
    }
    fn analysis_document(&self, id: Option<String>, source: Option<String>) -> Result<library::Document, String> {
        let mut doc = if let Some(id) = id {
            self.strategy(&id)?
        } else if source.is_some() {
            library::new_document(&self.root, 1)?
        } else {
            return Err("請提供 strategy_id 或 source".into());
        };
        if let Some(source) = source {
            set_source(&mut doc, source)?;
        }
        Ok(doc)
    }

    fn strategy(&self, id: &str) -> Result<library::Document, String> {
        let entry = library::catalog(&self.root)?
            .into_iter()
            .find(|entry| entry.id == id)
            .ok_or("策略庫找不到此 id")?;
        library::open(&self.root, &entry)
    }

    fn run_dir(&self, id: &str) -> Result<PathBuf, String> {
        if !valid_id(id, "rust-") {
            return Err("run_id 格式錯誤".into());
        }
        Ok(self.root.join("outputs/pl-workbench").join(id))
    }

    fn source_ref(&self, reference: &Value) -> Result<String, String> {
        let strategy_id = reference["strategy_id"].as_str();
        let run_id = reference["run_id"].as_str();
        let source = reference["source"].as_str();
        if [strategy_id, run_id, source].into_iter().flatten().count() != 1 {
            return Err("請在 strategy_id、run_id 與 source 中指定一項".into());
        }
        if let Some(id) = strategy_id { return Ok(self.strategy(id)?.source); }
        if let Some(id) = run_id { return recharts_core::pl_exchange::read(&self.run_dir(id)?.join("input.pl")); }
        Ok(source.unwrap_or_default().to_owned())
    }

    fn run(&self, doc: &library::Document, baseline: Option<&Value>) -> Result<Value, String> {
        self.check_cancelled()?;
        self.progress("準備策略與回測設定");
        let receipt = research::optimization_run_controlled(&self.root, doc, baseline, "mcp-stdio",
            |line| self.progress(&line), self.job.as_ref().map(|job| &job.stop))?;
        self.check_cancelled()?;
        let path = self.run_dir(receipt["run_id"].as_str().ok_or("缺少回測編號")?)?;
        workbench::write_json(&path.join("mcp-task.json"), &doc.task)?;
        Ok(receipt)
    }

    fn baseline(&self, id: &str) -> Result<(PathBuf, Value), String> {
        let dir = self.run_dir(id)?;
        let base = workbench::read_json(&dir.join("result.json"))?;
        if base["status"] != "completed"
            || base["ui_runtime"] != "mcp-stdio"
            || !base["optimization_inputs"].is_object()
        {
            return Err("請先使用 MCP 的 backtest_run 建立基準".into());
        }
        research::require_trade_validation(&base)?;
        Ok((dir, base))
    }

    fn validate_research_run(&self, id: &str) -> Result<(), String> {
        let receipt = workbench::read_json(&self.run_dir(id)?.join("result.json"))?;
        if receipt["status"] != "completed" { return Err("回測尚未完成".into()); }
        research::require_trade_validation(&receipt)
    }

    fn compare(&self, args: Candidate) -> Result<Value, String> {
        let (dir, base) = self.baseline(&args.baseline_run_id)?;
        let mut doc = library::new_document(&self.root, 1)?;
        doc.task = workbench::read_json(&dir.join("mcp-task.json"))?;
        let original = fs::read_to_string(dir.join("input.pl")).map_err(|e| e.to_string())?;
        if original.trim() == args.source.trim() {
            return Err("候選版與原版相同".into());
        }
        set_source(&mut doc, args.source)?;
        let candidate = self.run(&doc, Some(&base["optimization_inputs"]))?;
        let mut delta = json!({});
        for key in ["net_closed", "max_drawdown", "trades", "win_rate"] {
            let a = base["summary"][key].as_f64().ok_or("基準摘要不完整")?;
            let b = candidate["summary"][key].as_f64().ok_or("候選摘要不完整")?;
            delta[key] = json!(b - a);
        }
        Ok(json!({"baseline":base,"candidate":candidate,"delta":delta}))
    }

    fn call(&self, name: &str, args: Value) -> Result<Value, String> {
        match name {
            "api_profile_schema" | "api_profile_list" | "api_profile_save" => {
                recharts_core::api_profiles::call(&self.root, name, args)
            }
            "market_sources" => {
                let args: MarketSource = parse(args)?;
                let sources = canonical_market::sources(&self.market)?;
                let rows: Vec<_> = sources
                    .as_array()
                    .ok_or("來源清單格式錯誤")?
                    .iter()
                    .filter(|row| {
                        args.symbol.as_ref().is_none_or(|symbol| {
                            row["symbol"]
                                .as_str()
                                .is_some_and(|name| name.eq_ignore_ascii_case(symbol))
                        })
                    })
                    .collect();
                Ok(json!({"sources":rows}))
            }
            "market_query" => {
                let args: MarketQuery = parse(args)?;
                let limit = args.limit.unwrap_or(100);
                if !(1..=1000).contains(&limit)
                    || !["tick", "minute", "daily"].contains(&args.kind.as_str())
                {
                    return Err("來源須為 tick、minute 或 daily，筆數上限須為 1 至 1000".into());
                }
                let from = timestamp(&args.from, false)?;
                let to = timestamp(&args.to, true)?;
                if from > to {
                    return Err("起始時間晚於結束時間".into());
                }
                let mut conn = database::open_existing(&self.market)?;
                let tx = conn.transaction().map_err(|err| err.to_string())?;
                let id = canonical_market::resolve_in(&tx, &args.symbol, &args.kind)?;
                if args.kind == "tick" {
                    let revision: String = tx.query_row(
                        "SELECT COALESCE(sha256,'') || ':' || row_count FROM datasets WHERE id=?1",
                        [id], |row| row.get(0),
                    ).map_err(|err| err.to_string())?;
                    let after = if let Some(cursor) = &args.cursor {
                        if cursor.dataset_id != id || cursor.from != from || cursor.to != to
                            || cursor.ordinal < 1 || cursor.datetime < from || cursor.datetime > to
                            || timestamp(&cursor.datetime, false)? != cursor.datetime
                        {
                            return Err("續查游標與商品、查詢起訖或成交位置不符".into());
                        }
                        if cursor.revision != revision {
                            return Err("Tick 資料已更新，請從第一頁重新查詢".into());
                        }
                        Some((cursor.datetime.as_str(), cursor.ordinal))
                    } else { None };
                    let mut rows = recharts_core::tick_store::query_tick_page(&tx, id, &from, &to, limit + 1, after)?;
                    let truncated = rows.len() > limit;
                    rows.truncate(limit);
                    let next_cursor = rows.last().filter(|_| truncated).map(|last| TickCursor {
                        dataset_id: id, datetime: last.record.datetime.clone(), ordinal: last.ordinal,
                        from, to, revision,
                    });
                    return Ok(json!({"dataset_id":id,"records":rows,"truncated":truncated,"next_cursor":next_cursor}));
                }
                if args.cursor.is_some() { return Err("續查游標僅支援 Tick".into()); }
                let mut rows = database::query(&tx, id, &from, &to, limit + 1)?;
                let truncated = rows.len() > limit;
                rows.truncate(limit);
                Ok(json!({"dataset_id":id,"records":rows,"truncated":truncated}))
            }
            "strategy_list" => {
                if args != json!({}) {
                    return Err("此工具無參數".into());
                }
                let entries: Vec<_> = library::catalog(&self.root)?.into_iter()
                    .map(|entry| json!({"id":entry.id,"name":entry.name,"kind":entry.kind,"managed":entry.managed}))
                    .collect();
                Ok(json!({"strategies":entries}))
            }
            "strategy_read" => {
                let args: StrategyId = parse(args)?;
                let doc = self.strategy(&args.strategy_id)?;
                Ok(
                    json!({"id":doc.id,"name":doc.name,"kind":doc.kind,"source":doc.source,"task":doc.task}),
                )
            }
            "strategy_diff" => {
                let before = self.source_ref(&args["before"])?;
                let after = self.source_ref(&args["after"])?;
                recharts_core::pl_diff::compare_and_export(&self.root, &before, &after)
            }
            "strategy_export_pl" => {
                let source = self.source_ref(&args)?;
                if source.trim().is_empty() || source.len() > 100_000 {
                    return Err("原始碼須有內容，且不可超過 100,000 位元組".into());
                }
                recharts_core::pl_exchange::export(&self.root, &source)
            }
            "strategy_version" => {
                let id = args["strategy_id"].as_str().ok_or("缺少策略編號")?;
                let mut doc = self.strategy(id)?;
                if args["operation"] == "commit" {
                    if let Some(source) = args.get("source") {
                        set_source(&mut doc, source.as_str().ok_or("策略原碼須為文字")?.into())?;
                    }
                    if let Some(patch) = args.get("parameters") {
                        let patch = patch.as_object().ok_or("參數覆寫須為 JSON 物件")?;
                        let current = doc.task["parameters"].as_object_mut().ok_or("策略缺少回測參數")?;
                        current.extend(patch.clone());
                    }
                    if let Some(inputs) = args.get("inputs") {
                        doc.task["parameters"]["inputs"] = inputs.clone();
                    }
                } else if !matches!(args["operation"].as_str(), Some("list" | "read" | "draft" | "fetch" | "push")) {
                    return Err("未知的策略版本操作".into());
                }
                recharts_core::strategy_versions::execute(&self.root, doc, &args)
            }
            "strategy_check" => {
                let args: StrategyCheck = parse(args)?;
                research::check(&self.root, &self.analysis_document(args.strategy_id, args.source)?)
            }
            "strategy_requirements" => {
                let known = if let Some(id) = args["strategy_id"].as_str() {
                    Some(strategy_requirements::known_context(&self.strategy(id)?))
                } else {
                    args.get("draft_context").filter(|value| value.is_object()).cloned()
                };
                Ok(strategy_requirements::review(known.as_ref(), &args["acceptance"], args["request"].as_str().unwrap_or(""), args["mode"].as_str()))
            }
            "strategy_logic" => {
                let args: StrategyLogic = parse(args)?;
                let doc = self.analysis_document(args.strategy_id, args.source)?;
                recharts_core::strategy_logic::analyze(&self.root, &doc, args.export_html)
            }
            "backtest_run" => {
                let args: Backtest = parse(args)?;
                let mut doc = self.strategy(&args.strategy_id)?;
                if let Some(source) = args.source {
                    set_source(&mut doc, source)?;
                }
                if let Some(from) = args.from {
                    doc.task["parameters"]["from"] = json!(if from.trim().is_empty() {
                        String::new()
                    } else {
                        timestamp(&from, false)?
                    });
                }
                if let Some(to) = args.to {
                    doc.task["parameters"]["to"] = json!(timestamp(&to, true)?);
                }
                if let Some(capital) = args.initial_capital {
                    if !capital.is_finite() || capital <= 0.0 {
                        return Err("原始資金須為正數".into());
                    }
                    doc.task["parameters"]["initial_capital"] = json!(capital);
                }
                if let Some(warmup) = args.warmup_bars {
                    doc.task["parameters"]["warmup_bars"] = json!(warmup);
                }
                self.run(&doc, None)
            }
            "backtest_result" => {
                let args: BacktestResult = parse(args)?;
                let page = args.page.unwrap_or(1);
                if page == 0 || page > usize::MAX / 100 {
                    return Err("頁碼超出範圍".into());
                }
                let dir = self.run_dir(&args.run_id)?;
                let receipt = workbench::read_json(&dir.join("result.json"))?;
                let view = research::view(&receipt, page - 1)?;
                Ok(
                    json!({"receipt":view["receipt"],"page":page,"trades":view["trades"],"trade_count":view["trade_count"],
                    "bar_count":view["bar_count"],"open_trade":view["open_trade"]}),
                )
            }
            "robustness_run" => {
                let mut args = args;
                let run_id = args.as_object_mut().and_then(|fields| fields.remove("run_id"))
                    .and_then(|value| value.as_str().map(str::to_owned)).ok_or("缺少回測編號")?;
                self.progress("準備強健性檢測資料");
                recharts_core::robustness::run_controlled(&self.root, &run_id, parse(args)?,
                    |line| self.progress(&line), self.job.as_ref().map(|job| &job.stop))
            }
            "optimization_compare" => {
                let args: Candidate = parse(args)?;
                self.compare(args)
            }
            "report_read" => {
                let args: Report = parse(args)?;
                self.progress("讀取回測報表");
                let (dir, receipt, report) = self.completed_report(&args.run_id)?;
                self.check_cancelled()?;
                let stats = match receipt.get("report_statistics") {
                    Some(stats) if matches!(args.date_basis, performance::DateBasis::Auto) => stats.clone(),
                    _ => performance::performance(&report, &receipt["report_calendar"], args.date_basis)?,
                };
                let coverage = receipt.get("market_coverage").cloned().unwrap_or_else(|| json!({"status":"not_recorded"}));
                let validation = if receipt["trade_validation"].is_object() {
                    receipt["trade_validation"].clone()
                } else {
                    research::validate_closed_trades(&report["trades"])
                };
                let mut output = json!({
                    "run_id":args.run_id,"summary":report["summary"],"warnings":report["warnings"],
                    "first_bar":report["first_bar"],"last_bar":report["last_bar"],"bar_count":report["bar_count"],
                    "ambiguous_bars":report["ambiguous_bars"],"open_trade":report["open_trade"],
                    "provenance":report["provenance"],"engine_version":report["engine_version"],
                    "tick_execution":report["tick_execution"],"inputs":report["inputs"],
                    "settings":report["config"],"artifacts":receipt["artifacts"],"performance":stats,"trade_validation":validation,
                    "market_coverage":coverage,
                });
                if let Some(settings) = output["settings"].as_object_mut() {
                    settings.remove("functions");
                }
                if args.include_source {
                    output["source"] =
                        json!(fs::read_to_string(dir.join("input.pl"))
                            .map_err(|err| err.to_string())?);
                }
                if let Some(format) = args.format {
                    self.check_cancelled()?;
                    self.progress("匯出回測報表");
                    let mut supplement = stats;
                    supplement["market_coverage"] = coverage;
                    output["export"] = performance::export_mc(&self.root, &dir, format, args.style, &supplement,
                        self.job.as_ref().map(|job| &job.stop), &mut |line| self.progress(&line))?;
                }
                Ok(output)
            }
            "model_update_report" => {
                let args: UpdateReport = parse(args)?;
                self.progress("比較兩份回測報表");
                if args.baseline_run_id == args.candidate_run_id {
                    return Err("請指定兩個不同回測編號".into());
                }
                self.validate_research_run(&args.baseline_run_id)?;
                self.validate_research_run(&args.candidate_run_id)?;
                let (_, base_receipt, baseline) = self.completed_report(&args.baseline_run_id)?;
                let (_, new_receipt, candidate) = self.completed_report(&args.candidate_run_id)?;
                self.check_cancelled()?;
                performance::compare(&self.root,
                    (&args.baseline_run_id, &baseline, &base_receipt["report_calendar"]),
                    (&args.candidate_run_id, &candidate, &new_receipt["report_calendar"]),
                    args.format, args.date_basis)
            }
            name if name.starts_with("loop_") => self.loop_call(name, args),
            _ => Err(format!("找不到 MCP 工具：{name}")),
        }
    }

    fn handle(&mut self, msg: Value) -> Option<Value> {
        let id = msg.get("id");
        let method = msg["method"].as_str();
        if msg["jsonrpc"] != "2.0"
            || method.is_none()
            || id
                .is_some_and(|id| !id.is_string() && id.as_i64().is_none() && id.as_u64().is_none())
        {
            return Some(error(Value::Null, -32600, "無效的 JSON-RPC 請求"));
        }
        let method = method.unwrap();
        let Some(id) = id.cloned() else {
            if method == "notifications/initialized" && self.protocol.is_some() {
                self.ready = true;
            }
            return None;
        };
        let params = &msg["params"];
        let result = match method {
            "initialize" => {
                let Some(version) = params["protocolVersion"].as_str() else {
                    return Some(error(id, -32602, "缺少 protocolVersion"));
                };
                if self.protocol.is_some() {
                    return Some(error(id, -32600, "此連線已初始化"));
                }
                let version = PROTOCOLS
                    .into_iter()
                    .find(|supported| *supported == version)
                    .unwrap_or(PROTOCOLS[0]);
                self.protocol = Some(version);
                json!({"protocolVersion":version,"capabilities":{"tools":{}},
                    "serverInfo":{"name":"strategy_studio","version":env!("CARGO_PKG_VERSION")},
                    "instructions":format!("{RESEARCH_ACCEPTANCE}\n產出的 PowerLanguage 原始碼須包含繁體中文註釋，說明重要參數與交易邏輯。交付給使用者前，使用 strategy_export_pl 匯出 Windows Big5／CP950，使用 strategy_diff 提供原版和改版的逐行對照。\n策略研究流程：backtest_run 建立基準，report_read 讀報表，依使用者準則評分後呼叫 loop_start。依 loop_status 指示使用 loop_submit、report_read、loop_score，最後 loop_finish。使用 loop_id 可接續，評分由呼叫端負責。")})
            }
            "ping" => json!({}),
            _ if !self.ready => return Some(error(id, -32002, "請先完成 MCP 初始化")),
            "tools/list" => json!({"tools":tools()}),
            "tools/call" => {
                let Some(name) = params["name"].as_str() else {
                    return Some(error(id, -32602, "缺少工具名稱"));
                };
                let args = params
                    .get("arguments")
                    .cloned()
                    .unwrap_or_else(|| json!({}));
                if !args.is_object() {
                    return Some(error(id, -32602, "arguments 必須是物件"));
                }
                tool_result(self.call(name,args), self.protocol)
            }
            _ => return Some(error(id, -32601, "找不到 JSON-RPC 方法")),
        };
        Some(json!({"jsonrpc":"2.0","id":id,"result":result}))
    }
}

fn set_source(doc: &mut library::Document, source: String) -> Result<(), String> {
    if source.trim().is_empty() || source.len() > 100_000 {
        return Err("PL 原始碼須有內容，大小上限為 100000 位元組".into());
    }
    doc.source = source;
    Ok(())
}

fn error(id: Value, code: i32, message: &str) -> Value {
    json!({"jsonrpc":"2.0","id":id,"error":{"code":code,"message":message}})
}

fn tool_result(value: Result<Value, String>, protocol: Option<&str>) -> Value {
    match value {
        Ok(value) => {
            let mut result = json!({"content":[{"type":"text","text":value.to_string()}],"isError":false});
            if protocol != Some("2025-03-26") { result["structuredContent"] = value; }
            result
        }
        Err(message) => json!({"content":[{"type":"text","text":message}],"isError":true}),
    }
}

fn serve(root: &Path, market: PathBuf) -> io::Result<()> {
    let mut server = Server {
        root: root.into(),
        market,
        protocol: None,
        ready: false,
        job: None,
    };
    let output = jobs::Output::default();
    let mut jobs = jobs::Jobs::default();
    for line in io::stdin().lock().lines() {
        let line = line?;
        let response = match serde_json::from_str(&line) {
            Ok(value) => {
                if jobs.dispatch(&server, &value, &output)? { continue; }
                server.handle(value)
            }
            Err(_) => Some(error(Value::Null, -32700, "無法解析 JSON")),
        };
        if let Some(value) = response {
            output.write(&value)?;
        }
    }
    Ok(())
}

fn main() {
    let root = workbench::root();
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    let market = match args.as_slice() {
        [] => root.join("data/market.sqlite"),
        [flag, path] if flag == "--database" => {
            let path = PathBuf::from(path);
            if path.is_absolute() {
                path
            } else {
                root.join(path)
            }
        }
        _ => {
            eprintln!("用法：strategy_mcp.exe [--database 行情資料庫]");
            std::process::exit(2);
        }
    };
    if let Err(err) = serve(&root, market) {
        eprintln!("{err}");
        std::process::exit(1);
    }
}
