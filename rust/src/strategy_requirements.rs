use crate::strategy_library::Document;
use serde_json::{Value, json};
use std::{io::Write, path::Path, process::{Command, Stdio}};

pub const INSTRUCTIONS: &str = "新建策略時一律呼叫 strategy_requirements 並指定 mode=new，逐項確認商品、週期、交易時段與行情來源、策略目標、預期進場、預期出場、停損與停利方式。修改既有策略遇到模糊需求也呼叫此工具。原樣呈現 questions 中的題目與所有選項，以 recommended 標記建議答案，以 free_text 呈現自行描述。請勿自行增刪或改寫題目與選項。將已明確的條件及使用者答案按 key 填回 acceptance，再查剩餘問題。確認驗收清單後，取得使用者明確同意才撰寫 PL、回測或啟動研究迴圈。";

pub const NEW_FIELDS: [&str; 8] = [
    "product", "bar_period", "market_session", "strategy_goal",
    "entry_rules", "exit_rules", "stop_loss", "take_profit",
];

pub fn known_context(doc: &Document) -> Value {
    let config = &doc.task["parameters"];
    json!({"name":doc.name,"market":doc.task["market"],"from":config["from"],"to":config["to"],
        "warmup_bars":config["warmup_bars"],"commission_per_side":config["commission_per_side"],
        "slippage_ticks":config["slippage_ticks"],"contracts":config["contracts"],"inputs":config["inputs"]})
}

pub fn review(known: Option<&Value>, acceptance: &Value, request: &str, mode: Option<&str>) -> Value {
    let new_strategy = match mode {
        Some("new") => true,
        Some("existing") => false,
        _ => ["新策略","新建策略","新增策略","從零開始","製作策略","寫一支策略","設計策略"]
            .iter().any(|word| request.contains(word)),
    };
    let known = if new_strategy { None } else { known };
    let market_default = known.and_then(|info| {
        let market = &info["market"];
        let unit = match market["unit"].as_str()? {
            "seconds" => "秒 K", "minutes" => "分 K", "days" => "日 K", _ => return None,
        };
        Some(format!("沿用 {}、{} {} 與原行情來源", market["symbol"].as_str()?, market["period"].as_u64()?, unit))
    }).unwrap_or_else(|| "依自行指定的商品與週期，使用該商品的已登錄行情來源".into());
    let backtest_default = known.map(|info| {
        let bound = |key: &str, fallback: &str| info[key].as_str().filter(|s| !s.is_empty()).unwrap_or(fallback).replace('T'," ");
        let from = bound("from", "資料起點");
        let to = bound("to", "資料終點");
        format!("沿用原設定（{from} 至 {to}，暖機 {} 根，單邊手續費 {} 元，滑價 {} 跳）",
            info["warmup_bars"], info["commission_per_side"], info["slippage_ticks"])
    }).unwrap_or_else(|| "先訂開發期及樣本外期間，兩版採相同暖機與成本".into());
    let trade_default = if known.is_some() {
        "先保留原出場，只調整進場篩選"
    } else {
        "先定義進出場及停損停利，再決定是否允許反手或加碼"
    };
    let pass_question = if request.contains("更穩") {
        "怎麼判定策略變得更穩？"
    } else {
        "策略要達到哪些門檻才算通過驗收？"
    };
    let existing = [
        ("market", "商品、K 棒週期與行情來源", "要使用哪個商品、K 棒週期與行情來源？",
            market_default.as_str(), "依可用的歷史行情先比較適合的商品與週期"),
        ("trade_rules", "交易方向、進出場條件、停損停利與例外", "本次優先調整哪些交易規則？",
            trade_default, "優先檢查停損、停利與其他出場條件"),
    ];
    let new = [
        (NEW_FIELDS[0], "交易商品", "新策略要用在哪個商品？請確認商品代碼。",
            "台指期近月（TXF1）", "小型臺指期貨連續月（MXF）"),
        (NEW_FIELDS[1], "K 棒週期", "要使用哪種 K 棒與週期？",
            "10 分 K", "5 分 K"),
        (NEW_FIELDS[2], "交易時段與行情來源", "要用哪個行情來源，涵蓋哪些交易時段？",
            "使用商品已登錄的歷史行情，涵蓋日盤與夜盤", "使用商品已登錄的歷史行情，僅限日盤"),
        (NEW_FIELDS[3], "策略目標", "這支策略的主要目標與適用情境是什麼？",
            "以樣本外回撤控制及交易穩定性為優先", "以扣除成本後的淨利為優先，同時觀察回撤"),
        (NEW_FIELDS[4], "預期進場方式", "是否已有預期進場方式？可說明多空方向、訊號和下單時點。",
            "尚無指定，由研究流程提出可驗收的進場條件", "以已收 K 棒確認突破，次棒才掛進場單"),
        (NEW_FIELDS[5], "預期出場方式", "除了停損停利，是否有預期的出場或強制平倉方式？",
            "以反向訊號出場，次棒才平倉", "以持倉時間或交易時段結束作為出場條件"),
        (NEW_FIELDS[6], "停損方式", "停損要如何觸發？若已有規則，可指定計算基準與參數。",
            "依近期波動設定停損，參數在驗收前確認", "依進場價格的百分比設定停損，比例在驗收前確認"),
        (NEW_FIELDS[7], "停利方式", "停利要如何觸發？若已有規則，可指定目標與出場方式。",
            "採移動停利，觸發與追蹤條件在驗收前確認", "以初始停損距離的倍數設定停利，倍數在驗收前確認"),
    ];
    let validation = [
        ("backtest", "回測期間、暖機、手續費與滑價", "回測期間、暖機與成本要如何設定？",
            backtest_default.as_str(), "保留同一套成本及暖機，另劃分開發期及樣本外期間"),
        ("pass_criteria", "評估指標、通過門檻或比較基準", pass_question,
            "樣本外最大回撤低於基準，扣成本淨利不低於基準", "以樣本外最大回撤為主，另列淨利與交易筆數"),
        ("evidence", "報表、逐筆成交或其他驗收證據", "需要交付哪些驗收證據？",
            "提供各輪比較、最佳版本、報表與逐筆跨棒驗收", "提供原版與最佳版的對照報表及逐筆跨棒驗收"),
    ];
    let mut checklist = Vec::new();
    let mut questions = Vec::new();
    let setup = if new_strategy { new.as_slice() } else { existing.as_slice() };
    for &(key, item, question, recommended, alternative) in setup.iter().chain(&validation) {
        let criterion = acceptance[key].as_str().unwrap_or("").trim();
        checklist.push(json!({"key":key,"item":item,"criterion":criterion,"needs_definition":criterion.is_empty()}));
        if criterion.is_empty() {
            questions.push(json!({"key":key,"question":question,"options":[
                {"id":"recommended","label":recommended,"recommended":true,"free_text":false},
                {"id":"alternative","label":alternative,"recommended":false,"free_text":false},
                {"id":"custom","label":"自行描述","recommended":false,"free_text":true}
            ]}));
        }
    }
    json!({"status":"review_required","mode":if new_strategy {"new"}else{"existing"},
        "known":known,"checklist":checklist,"questions":questions,
        "mandatory":"每筆平倉成交的 exit_bar 必須大於 entry_bar","next_action":INSTRUCTIONS})
}

// The desktop uses the same MCP executable and tool protocol as other clients.
pub(crate) fn query_local_mcp(root: &Path, arguments: Value) -> Result<Value, String> {
    let mut command = Command::new(root.join("rust/target/debug/strategy_mcp.exe"));
    command.current_dir(root).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    let mut child = command.spawn().map_err(|e| format!("無法啟動策略 MCP：{e}"))?;
    let send = (|| -> std::io::Result<()> {
        let mut input = child.stdin.take().expect("piped stdin");
        let requests = [
            json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{
                "protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"strategy-workbench","version":"1"}}}),
            json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
            json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"strategy_requirements","arguments":arguments}}),
        ];
        for request in requests {
            writeln!(input, "{request}")?;
        }
        Ok(())
    })();
    let output = child.wait_with_output().map_err(|e| e.to_string())?;
    send.map_err(|e| format!("無法送出策略 MCP 請求：{e}"))?;
    if !output.status.success() {
        return Err(format!("策略 MCP 啟動失敗（{}）", output.status));
    }
    let reply = String::from_utf8_lossy(&output.stdout).lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .find(|reply| reply["id"] == 2).ok_or("策略 MCP 未回傳驗收結果")?;
    if let Some(result) = reply["result"]["structuredContent"].as_object() {
        return Ok(json!(result));
    }
    Err(reply["result"]["content"][0]["text"].as_str()
        .or_else(|| reply["error"]["message"].as_str())
        .unwrap_or("策略 MCP 回覆格式不符").to_owned())
}
