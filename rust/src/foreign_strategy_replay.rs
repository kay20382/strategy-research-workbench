//! 225M validation through the TradeCom gateway's isolated paper ledger.
use crate::{
    powerlanguage::{Action, Kind, OrderSpec, Stmt},
    strategy_library, strategy_research,
    trading::Session,
    workbench,
};
use chrono::NaiveDate;
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

fn read_orders<'a>(
    body: &'a [Stmt],
    into: &mut BTreeMap<&'a str, &'a OrderSpec>,
) -> Result<(), String> {
    for statement in body {
        match statement {
            Stmt::Order(order) => {
                if order.total || into.insert(&order.label, order).is_some() {
                    return Err("海外回放要求每條委託具有唯一名稱，且使用單口非 Total 委託".into());
                }
            }
            Stmt::If(_, yes, no) => {
                read_orders(yes, into)?;
                read_orders(no, into)?;
            }
            Stmt::For(_, _, _, _, body) => read_orders(body, into)?,
            _ => {}
        }
    }
    Ok(())
}

fn receive(
    session: &mut Session,
    id: &str,
    kind: &str,
    stop: &AtomicBool,
) -> Result<Value, String> {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if stop.load(Ordering::Acquire) {
            return Err("已停止海外策略回放".into());
        }
        let (events, active) = session.poll()?;
        for event in events {
            if event["type"] == "status" && event["data"]["state"] == "error" {
                return Err(event["data"]["message"]
                    .as_str()
                    .unwrap_or("模擬連線異常")
                    .into());
            }
            if event["type"] == kind && (id.is_empty() || event["data"]["id"] == id) {
                return Ok(event["data"].clone());
            }
            if event["type"] == "reply" && event["data"]["id"] == id {
                return Err(event["data"]["message"]
                    .as_str()
                    .unwrap_or("模擬委託遭拒")
                    .into());
            }
        }
        if !active {
            return Err("模擬閘道連線已關閉".into());
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    Err(format!("等待模擬回報逾時：{id}"))
}

fn verify_position(
    session: &mut Session,
    id: &str,
    maturity: &str,
    expected: i64,
    stop: &AtomicBool,
) -> Result<(), String> {
    session.submit(json!({"id":id,"action":"positions"}))?;
    let reply = receive(session, id, "query", stop)?;
    let rows = reply["rows"].as_array().ok_or("模擬庫存回覆格式不符")?;
    if reply["code"] != 0
        || (expected == 0 && !rows.is_empty())
        || (expected != 0
            && (rows.len() != 1
                || rows[0]["symbol"] != format!("PAPER:225M:{maturity}")
                || rows[0]["signed_quantity"] != expected))
    {
        return Err(format!("模擬庫存與策略不一致，策略預期 {expected} 口"));
    }
    Ok(())
}

pub(crate) fn run(
    root: &Path,
    args: &Value,
    stop: &AtomicBool,
    mut progress: impl FnMut(String),
) -> Result<Value, String> {
    let date = |key| {
        NaiveDate::parse_from_str(args[key].as_str().ok_or("回放須提供起訖日期")?, "%Y-%m-%d")
            .map_err(|_| "回放日期須為 YYYY-MM-DD".to_owned())
    };
    let from = date("from")?;
    let to = date("to")?;
    if !(0..31).contains(&(to - from).num_days()) {
        return Err("每次回放須為 1 至 31 個曆日".into());
    }
    let entry = strategy_library::catalog(root)?
        .into_iter()
        .find(|e| {
            e.name == "REA_225M_Intraday"
                && e.source == root.join("strategies/REA_225M_Intraday.pl")
        })
        .ok_or("找不到原始 REA_225M_Intraday 策略")?;
    let mut document = strategy_library::open(root, &entry)?;
    if let Some(database) = std::env::var_os("STRATEGY_HISTORICAL_MARKET_DB").filter(|v| !v.is_empty()) {
        document.task["database"] = json!(root.join(database));
    }
    let program = strategy_research::compile_document(root, &document)?;
    let mut orders = BTreeMap::new();
    read_orders(&program.body, &mut orders)?;
    let begin = from.and_hms_opt(0, 0, 0).unwrap();
    let end = to.and_hms_opt(23, 59, 59).unwrap();
    let periods = if args["quarterly"] == true {
        quarterly_periods(begin, end)?
    } else {
        vec![(begin, end, "209912".to_owned())]
    };
    let mut session = Session::start_replay(root)?;
    receive(&mut session, "", "status", stop)?;
    let run = strategy_library::fresh_id();
    let mut segments = Vec::new();
    let mut rolls = Vec::new();
    for (index, (start, end, maturity)) in periods.iter().enumerate() {
        document.task["parameters"]["from"] = json!(start.format("%Y-%m-%dT%H:%M:%S").to_string());
        document.task["parameters"]["to"] = json!(end.format("%Y-%m-%dT%H:%M:%S").to_string());
        let prefix = format!("r-{run}-{index}-");
        let next = periods.get(index + 1);
        let (segment, rollover) = replay_segment(
            root,
            &document,
            &orders,
            &mut session,
            &prefix,
            maturity,
            next.map(|(date, _, _)| *date),
            stop,
            &mut progress,
        )?;
        segments.push(segment);
        if let (Some(mut rollover), Some((_, _, next_maturity))) = (rollover, next) {
            rollover["next_month"] = json!(next_maturity);
            rolls.push(rollover);
        }
    }
    let mut proof = segments.last().cloned().ok_or("缺少回放區段")?;
    if segments.len() > 1 {
        let mut by_label: BTreeMap<String, u64> = BTreeMap::new();
        for segment in &segments {
            if let Some(labels) = segment["fills_by_label"].as_object() {
                for (label, count) in labels {
                    *by_label.entry(label.clone()).or_default() += count.as_u64().unwrap_or(0);
                }
            }
        }
        proof["fills_by_label"] = json!(by_label);
        for key in [
            "order_count",
            "fill_count",
            "closed_without_fill",
            "position_checks",
        ] {
            proof[key] = json!(segments
                .iter()
                .filter_map(|row| row[key].as_u64())
                .sum::<u64>());
        }
        let count = segments
            .iter()
            .filter_map(|row| row["trade_validation"]["checked_trades"].as_u64())
            .sum::<u64>();
        proof["trade_validation"]["checked_trades"] = json!(count);
    }
    proof["segments"] = json!(segments);
    proof["rollovers"] = json!(rolls);
    proof["from"] = json!(from.to_string());
    proof["to"] = json!(to.to_string());
    proof["session_prefix"] = json!(format!("r-{run}-"));
    let dir = root.join("outputs/foreign-strategy-replay").join(&run);
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join("verification.json");
    proof["report_path"] = json!(path);
    workbench::write_json(&path, &proof)?;
    Ok(proof)
}

fn quarterly_periods(
    start: chrono::NaiveDateTime,
    end: chrono::NaiveDateTime,
) -> Result<Vec<(chrono::NaiveDateTime, chrono::NaiveDateTime, String)>, String> {
    let mut periods = Vec::new();
    let mut begin = start;
    while begin <= end {
        let contract = crate::trading::nikkei_quarter(begin)?;
        let maturity = contract["maturity"].as_str().ok_or("缺少日經季月")?;
        let roll = NaiveDate::parse_from_str(
            contract["roll_date_jst"].as_str().ok_or("缺少換月日")?,
            "%Y-%m-%d",
        )
        .map_err(|e| e.to_string())?
        .and_hms_opt(8, 45, 0)
        .unwrap();
        if roll <= begin {
            return Err("日經季月換月日期不一致".into());
        }
        periods.push((
            begin,
            end.min(roll - chrono::Duration::seconds(1)),
            maturity.to_owned(),
        ));
        begin = roll;
    }
    Ok(periods)
}

fn replay_segment(
    root: &Path,
    document: &strategy_library::Document,
    orders: &BTreeMap<&str, &OrderSpec>,
    mut session: &mut Session,
    prefix: &str,
    maturity: &str,
    cutover: Option<chrono::NaiveDateTime>,
    stop: &AtomicBool,
    progress: &mut dyn FnMut(String),
) -> Result<(Value, Option<Value>), String> {
    let receipt = workbench::run_controlled(
        root,
        document.task.clone(),
        &document.source,
        progress,
        stop,
    )?;
    if receipt["status"] != "completed" {
        return Err(receipt["error"]
            .as_str()
            .unwrap_or("225M 歷史行情回放失敗")
            .into());
    }
    strategy_research::require_trade_validation(&receipt)?;
    let report = workbench::read_json(Path::new(
        receipt["artifacts"]["json"]
            .as_str()
            .ok_or("缺少策略回放明細")?,
    ))?;
    let events = report["events"].as_array().ok_or("缺少委託事件")?;
    let fills = report["fills"].as_array().ok_or("缺少成交資料")?;
    let equity = report["equity"].as_array().ok_or("缺少策略逐棒部位")?;
    let mut fills_by_order = BTreeMap::new();
    for fill in fills {
        let id = fill["order_id"].as_u64().ok_or("成交缺少委託編號")?;
        if fills_by_order.insert(id, fill).is_some() || fill["quantity"] != 1 {
            return Err("225M 回放僅支援每張委託單一口成交".into());
        }
    }
    let command = |suffix: &str| format!("{prefix}{suffix}");
    verify_position(&mut session, &command("initial"), maturity, 0, stop)?;
    let mut working = BTreeSet::new();
    let mut submitted = BTreeSet::new();
    let mut executed = BTreeSet::new();
    let mut by_label: BTreeMap<String, usize> = BTreeMap::new();
    let mut position = 0;
    let mut closed = 0;
    let mut reconciliation = 1;
    for event in events {
        let number = event["order_id"].as_u64().ok_or("委託事件缺少編號")?;
        let label = event["label"].as_str().ok_or("委託事件缺少名稱")?;
        let spec = orders.get(label).ok_or("找不到原策略委託定義")?;
        let state = event["status"].as_str().ok_or("委託事件缺少狀態")?;
        let reference = command(&format!("o{number}"));
        let side = if matches!(spec.action, Action::Buy | Action::BuyToCover) {
            "buy"
        } else {
            "sell"
        };
        let entry = matches!(spec.action, Action::Buy | Action::SellShort);
        if submitted.insert(number) {
            let kind = match spec.kind {
                Kind::Market => "market",
                Kind::Limit => "limit",
                Kind::Stop => "stop",
                Kind::Close => "close",
            };
            let price = event["raw_order_price"]
                .as_f64()
                .map(|v| v.to_string())
                .unwrap_or_default();
            session.submit(json!({"id":reference,"action":"place","exchange":"PAPER","product":"225M",
                "maturity":maturity,"side":side,"position_effect":if entry {"open"} else {"close"},
                "price_type":kind,"time_in_force":"ioc","price":price,"quantity":"1","label":label}))?;
            let accepted = receive(&mut session, &reference, "order_report", stop)?;
            if accepted["code"] != 0 || accepted["simulated"] != true {
                return Err("模擬委託未確認".into());
            }
            working.insert(number);
        }
        match state {
            "generated" => {}
            "filled" => {
                let fill = fills_by_order
                    .get(&number)
                    .ok_or("已成交委託缺少策略成交記錄")?;
                if !working.remove(&number)
                    || !executed.insert(number)
                    || fill["label"] != label
                    || fill["is_entry"] != entry
                {
                    return Err("策略委託與成交事件不一致".into());
                }
                let id = command(&format!("f{number}"));
                let price = fill["price"].as_f64().ok_or("成交缺少價格")?;
                session.submit(
                    json!({"id":id,"action":"replay_execution","reference":reference,"quantity":"1",
                    "price":price.to_string(),"time":fill["datetime"],"label":label}),
                )?;
                let actual = receive(&mut session, &id, "fill", stop)?;
                if actual["reference"] != reference
                    || actual["quantity"] != 1
                    || actual["side"] != side
                    || actual["price"].as_str().and_then(|v| v.parse::<f64>().ok()) != Some(price)
                {
                    return Err("模擬成交回報與策略預期不符".into());
                }
                position += if side == "buy" { 1 } else { -1 };
                let bar = fill["bar"].as_u64().ok_or("成交缺少 K 棒編號")? as usize;
                if equity.get(bar).and_then(|row| row["position"].as_i64()) != Some(position) {
                    return Err(format!("第 {bar} 棒的策略部位與成交變動不一致"));
                }
                verify_position(
                    &mut session,
                    &command(&format!("p{number}")),
                    maturity,
                    position,
                    stop,
                )?;
                reconciliation += 1;
                *by_label.entry(label.into()).or_default() += 1;
            }
            "expired" | "ignored_or_rejected" => {
                if !working.remove(&number) {
                    return Err("委託終止事件與模擬帳冊不符".into());
                }
                let id = command(&format!("x{number}"));
                session.submit(json!({"id":id,"action":"replay_close_order","reference":reference,"reason":state}))?;
                receive(&mut session, &id, "order_report", stop)?;
                closed += 1;
            }
            _ => return Err(format!("無法重播的委託狀態：{state}")),
        }
    }
    if working.len() != report["pending_at_end"].as_u64().unwrap_or(u64::MAX) as usize
        || executed.len() != fills.len()
        || report["final_position"] != position
    {
        return Err("期末的委託、成交或部位與策略結果不同".into());
    }
    verify_position(&mut session, &command("final"), maturity, position, stop)?;
    let mut rollover = None;
    let mut rollover_fill = 0;
    if let Some(cutover) = cutover {
        for number in std::mem::take(&mut working) {
            let id = command(&format!("rollx{number}"));
            session.submit(json!({"id":id,"action":"replay_close_order","reference":command(&format!("o{number}")),"reason":"quarter_roll"}))?;
            receive(&mut session, &id, "order_report", stop)?;
            closed += 1;
        }
        let old_position = position;
        if position != 0 {
            let file = receipt["artifacts"]["bars"]
                .as_str()
                .ok_or("缺少舊月行情快照")?;
            let mut reader = csv::Reader::from_path(file).map_err(|e| e.to_string())?;
            let last = reader
                .deserialize::<crate::backtest::Candle>()
                .last()
                .ok_or("缺少舊月收盤價")?
                .map_err(|e| e.to_string())?;
            let time = cutover.format("%Y-%m-%dT%H:%M:%S").to_string();
            close_for_rollover(
                &mut session,
                prefix,
                maturity,
                position,
                last.close,
                &time,
                stop,
            )?;
            position = 0;
            rollover_fill = 1;
        }
        verify_position(&mut session, &command("roll-flat"), maturity, 0, stop)?;
        reconciliation += 1;
        let id = command("roll-orders");
        session.submit(json!({"id":id,"action":"orders"}))?;
        let open = receive(&mut session, &id, "query", stop)?;
        if open["code"] != 0 || open["rows"].as_array().is_none_or(|rows| !rows.is_empty()) {
            return Err("舊月仍有未結委託，停止換倉".into());
        }
        rollover = Some(
            json!({"old_month":maturity,"cutover_jst":cutover.format("%Y-%m-%dT%H:%M:%S").to_string(),"old_position":old_position,
            "close_fills":rollover_fill,"verified_flat":true}),
        );
    }
    let proof = json!({"mode":"paper","strategy":document.name,"run_id":receipt["run_id"],"maturity":maturity,
        "order_count":submitted.len()+rollover_fill,"fill_count":fills.len()+rollover_fill,
        "closed_without_fill":closed,"pending_at_end":working.len(),"position_checks":reconciliation+1,
        "final_position":position,"fills_by_label":by_label,"trade_validation":receipt["trade_validation"],
        "simulation":"連續歷史 OHLC 為模擬價格，海外 paper 閘道獨立記帳與回報"});
    Ok((proof, rollover))
}

fn close_for_rollover(
    session: &mut Session,
    prefix: &str,
    maturity: &str,
    position: i64,
    price: f64,
    time: &str,
    stop: &AtomicBool,
) -> Result<(), String> {
    let side = if position > 0 { "sell" } else { "buy" };
    let reference = format!("{prefix}roll-close");
    session.submit(
        json!({"id":reference,"action":"place","exchange":"PAPER","product":"225M",
        "maturity":maturity,"side":side,"position_effect":"close","price_type":"market",
        "time_in_force":"ioc","quantity":"1","label":"QuarterRollClose"}),
    )?;
    let accepted = receive(session, &reference, "order_report", stop)?;
    if accepted["code"] != 0 || accepted["simulated"] != true {
        return Err("舊月平倉委託未確認，停止換倉".into());
    }
    let id = format!("{prefix}roll-fill");
    session.submit(
        json!({"id":id,"action":"replay_execution","reference":reference,"price":price.to_string(),
        "quantity":"1","time":time,"label":"QuarterRollClose"}),
    )?;
    let fill = receive(session, &id, "fill", stop)?;
    if fill["reference"] != reference
        || fill["side"] != side
        || fill["quantity"] != 1
        || fill["simulated"] != true
        || fill["symbol"] != format!("PAPER:225M:{maturity}")
        || fill["remaining"] != 0
    {
        return Err("舊月平倉成交未確認，停止換倉".into());
    }
    Ok(())
}

#[cfg(test)]
#[test]
fn quarterly_replay_confirms_old_fill_and_flat_before_new_month() {
    let start = NaiveDate::from_ymd_opt(2026, 6, 8)
        .unwrap()
        .and_hms_opt(0, 0, 0)
        .unwrap();
    let end = NaiveDate::from_ymd_opt(2026, 6, 12)
        .unwrap()
        .and_hms_opt(23, 59, 59)
        .unwrap();
    let periods = quarterly_periods(start, end).unwrap();
    assert_eq!(periods.len(), 2);
    assert_eq!((&*periods[0].2, &*periods[1].2), ("202606", "202609"));
    assert_eq!(periods[1].0.to_string(), "2026-06-10 08:45:00");
    let root = workbench::root()
        .join("outputs")
        .join(format!("nikkei-roll-test-{}", strategy_library::fresh_id()));
    let gateway = root.join("outputs/trade-gateway");
    fs::create_dir_all(&gateway).unwrap();
    for file in fs::read_dir(workbench::root().join("outputs/trade-gateway")).unwrap() {
        let file = file.unwrap();
        if matches!(
            file.path().extension().and_then(|v| v.to_str()),
            Some("exe" | "dll")
        ) {
            fs::copy(file.path(), gateway.join(file.file_name())).unwrap();
        }
    }
    // The root uses its own simulation journal unless a caller explicitly configured one.
    assert!(std::env::var_os("STRATEGY_TRADE_JOURNAL").is_none());
    let stop = AtomicBool::new(false);
    let mut session = Session::start_replay(&root).unwrap();
    receive(&mut session, "", "status", &stop).unwrap();
    let open = |session: &mut Session, id: &str, maturity: &str| {
        session.submit(json!({"id":id,"action":"place","exchange":"PAPER","product":"225M","maturity":maturity,
            "side":"buy","position_effect":"open","price_type":"market","time_in_force":"ioc","quantity":"1"})).unwrap();
        receive(session, id, "order_report", &stop).unwrap();
        let fill = format!("{id}-fill");
        session.submit(json!({"id":fill,"action":"replay_execution","reference":id,"price":"39000","quantity":"1"})).unwrap();
        receive(session, &fill, "fill", &stop)
    };
    open(&mut session, "old", "202606").unwrap();
    verify_position(&mut session, "old-before", "202606", 1, &stop).unwrap();
    assert!(close_for_rollover(
        &mut session,
        "bad-",
        "202606",
        -1,
        39005.0,
        "2026-06-10T08:45:00",
        &stop
    )
    .is_err());
    verify_position(&mut session, "not-flat", "202606", 1, &stop).unwrap();
    assert!(open(&mut session, "early", "202609").is_err());
    close_for_rollover(
        &mut session,
        "ok-",
        "202606",
        1,
        39005.0,
        "2026-06-10T08:45:00",
        &stop,
    )
    .unwrap();
    verify_position(&mut session, "flat", "202606", 0, &stop).unwrap();
    open(&mut session, "new", "202609").unwrap();
    verify_position(&mut session, "new-position", "202609", 1, &stop).unwrap();
}
