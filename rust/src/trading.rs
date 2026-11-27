use crate::{api_connection::{self, Indicator, RETRY_DELAY}, api_profiles, workbench};
use godot::prelude::*;
use rusqlite::{params, Connection};
use serde_json::{json, Value};
#[cfg(windows)]
use std::os::windows::process::CommandExt;
use std::{
    fs,
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    process::{Child, ChildStdin, Command, Stdio},
    sync::mpsc::{self, Receiver, TryRecvError},
    time::{Duration, Instant},
};

pub struct Session {
    child: Child,
    input: Option<ChildStdin>,
    events: Receiver<Result<Value, String>>,
    journal: Connection,
    session: String,
    profile: String,
    mode: String,
    pub ready: bool,
    replay: bool,
    retryable: bool,
    recovery_ready: bool,
    reset_at: Option<chrono::DateTime<chrono::Utc>>,
    closing: Option<Instant>,
    light: Option<Indicator>,
}

pub(crate) fn journal_path(root: &Path) -> std::path::PathBuf {
    std::env::var_os("STRATEGY_TRADE_JOURNAL")
        .filter(|v| !v.is_empty())
        .map(|v| root.join(v))
        .unwrap_or_else(|| root.join("data/trading.sqlite"))
}

fn database(root: &Path) -> Result<Connection, String> {
    let path = journal_path(root);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let db = Connection::open(path).map_err(|e| e.to_string())?;
    db.busy_timeout(Duration::from_secs(3))
        .map_err(|e| e.to_string())?;
    db.execute_batch(
        "CREATE TABLE IF NOT EXISTS trade_commands(
         profile TEXT NOT NULL, mode TEXT NOT NULL, id TEXT NOT NULL,
         session TEXT NOT NULL, request TEXT NOT NULL,
         recorded_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP, PRIMARY KEY(profile,mode,id));
         CREATE TABLE IF NOT EXISTS trade_events(
         seq INTEGER PRIMARY KEY, profile TEXT NOT NULL, mode TEXT NOT NULL,
         session TEXT NOT NULL, event TEXT NOT NULL,
         received_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP);",
    )
    .map_err(|e| e.to_string())?;
    Ok(db)
}

fn configure(command: &mut Command, root: &Path, id: &str) -> Result<String, String> {
    let (profile, mut credentials) = api_profiles::load(&api_profiles::location(root), id)?;
    if profile.provider != "kgi_tradecom" || profile.purpose != "trade" {
        return Err("請選擇凱基 TradeCom 交易設定".into());
    }
    for (key, label, value) in [
        ("HOST", "主機", Some(profile.host)),
        ("PORT", "連接埠", profile.port.map(|port| port.to_string())),
        ("USER", "使用者", credentials.account),
        ("PASSWORD", "密碼", credentials.password),
        (
            "BROKER",
            "券商代碼",
            profile.options.get("broker_id").cloned(),
        ),
        (
            "ACCOUNT",
            "交易帳號",
            credentials.secrets.remove("trade_account"),
        ),
    ] {
        let setting = value
            .filter(|v| !v.trim().is_empty())
            .ok_or_else(|| format!("交易設定缺少{label}"))?;
        command.env(format!("TRADECOM_{key}"), setting);
    }
    command.env(
        "TRADECOM_SUB_ACCOUNT",
        credentials
            .secrets
            .remove("sub_account")
            .unwrap_or_default(),
    );
    command.env(
        "TRADECOM_GROUP",
        profile.options.get("group").cloned().unwrap_or_default(),
    );
    Ok(profile.id)
}

impl Session {
    pub fn start(root: &Path, id: &str, mode: &str) -> Result<Self, String> {
        Self::start_market(root, id, mode, false, false)
    }

    pub(crate) fn start_replay(root: &Path) -> Result<Self, String> {
        Self::start_market(root, "", "paper", true, true)
    }

    fn start_market(root: &Path, id: &str, mode: &str, foreign: bool, replay: bool) -> Result<Self, String> {
        if !["paper", "read_only", "live"].contains(&mode) {
            return Err("交易模式須為 paper、read_only 或 live".into());
        }
        let gateway = root.join("outputs/trade-gateway/TradeGateway.exe");
        if !gateway.is_file() {
            return Err("找不到交易轉接器，請先執行 trade-gateway.ps1".into());
        }
        let mut command = Command::new(&gateway);
        let profile = if mode == "paper" {
            "paper".to_owned()
        } else {
            configure(&mut command, root, id)?
        };
        let journal = database(root)?;
        #[cfg(windows)]
        command.creation_flags(0x0800_0000);
        command.arg(mode);
        if foreign {
            command.arg("foreign");
        }
        if replay {
            command.arg("replay");
        }
        let mut child = command
            .current_dir(gateway.parent().expect("轉接器有父目錄"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("無法啟動交易轉接器：{e}"))?;
        let input = child.stdin.take();
        let output = child.stdout.take().expect("已建立交易回報管線");
        let (tx, events) = mpsc::sync_channel(512);
        std::thread::spawn(move || {
            for line in BufReader::new(output).split(b'\n') {
                let event = match line {
                    Ok(line) => {
                        let Some(prefix) = line.windows(6).position(|part| part == b"TRADE:")
                        else {
                            continue;
                        };
                        serde_json::from_slice::<Value>(&line[prefix + 6..])
                            .map_err(|_| "交易回報格式無效".to_owned())
                    }
                    Err(_) => Err("交易回報讀取失敗".to_owned()),
                };
                if tx.send(event).is_err() {
                    break;
                }
            }
        });
        let light = (mode != "paper").then(|| Indicator::new(root, "trade", &profile));
        Ok(Self {
            child,
            input,
            events,
            journal,
            session: crate::strategy_library::fresh_id(),
            profile,
            mode: mode.into(),
            ready: false,
            replay,
            retryable: mode != "paper",
            recovery_ready: mode == "read_only",
            reset_at: (mode != "paper").then(|| api_connection::next_reset(api_connection::utc_now())),
            closing: None,
            light,
        })
    }

    pub(crate) fn reconnectable(&self) -> bool { self.retryable }
    pub(crate) fn scheduled_reconnect(&self) -> bool { self.closing.is_some() }

    pub fn submit(&mut self, request: Value) -> Result<(), String> {
        if !self.ready || self.reset_at.is_some_and(|at| api_connection::utc_now() >= at) {
            return Err("交易連線尚未就緒".into());
        }
        let action = request["action"].as_str().ok_or("缺少交易操作")?;
        let order = matches!(
            action,
            "place" | "cancel" | "modify_price" | "modify_quantity"
        );
        let replay_action = matches!(action, "replay_execution" | "replay_close_order");
        if replay_action && !self.replay {
            return Err("模擬撮合指令僅限海外回放".into());
        }
        if !order && !replay_action && !matches!(action, "account" | "positions" | "margin" | "orders") {
            return Err("不支援的交易操作".into());
        }
        if order && self.mode == "read_only" {
            return Err("唯讀交易連線只接受查詢".into());
        }
        if order && self.mode == "live" && request["confirmed"] != true {
            return Err("請確認正式交易委託內容".into());
        }
        let id = request["id"]
            .as_str()
            .filter(|id| {
                !id.is_empty()
                    && id.len() <= 64
                    && id
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
            })
            .ok_or("交易指令編號格式錯誤")?;
        let mut sanitized = serde_json::Map::new();
        for key in [
            "id",
            "action",
            "confirmed",
            "product",
            "maturity",
            "side",
            "price_type",
            "price",
            "quantity",
            "time_in_force",
            "position_effect",
            "reference",
            "expected_symbol",
            "exchange",
            "label",
            "time",
            "reason",
        ] {
            if let Some(value) = request.get(key) {
                sanitized.insert(key.to_owned(), value.clone());
            }
        }
        let body = Value::Object(sanitized).to_string();
        if body.len() > 16384 {
            return Err("交易指令過長".into());
        }
        let written=self.journal.execute(
            "INSERT OR IGNORE INTO trade_commands(profile,mode,id,session,request) VALUES(?1,?2,?3,?4,?5)",
            params![self.profile,self.mode,id,self.session,body],
        ).map_err(|e|format!("交易日誌寫入失敗：{e}"))?;
        if written == 0 {
            return Err("交易指令編號已使用，請先核對原指令回報".into());
        }
        let input = self.input.as_mut().ok_or("交易管線已關閉")?;
        if writeln!(input, "{body}")
            .and_then(|_| input.flush())
            .is_err()
        {
            self.ready = false;
            return Err("交易指令已記錄，傳輸狀態未知。請先核對券商回報".into());
        }
        Ok(())
    }

    pub fn poll(&mut self) -> Result<(Vec<Value>, bool), String> {
        let mut batch = Vec::new();
        if self.closing.is_none() && self.reset_at.is_some_and(|at| api_connection::utc_now() >= at) {
            self.ready = false;
            self.input.take();
            self.closing = Some(Instant::now() + Duration::from_secs(5));
            if let Some(light) = &mut self.light { light.update("reconnecting"); }
            batch.push(json!({"type":"status","mode":self.mode,"data":{
                "state":"reconnecting","scheduled":true,"message":"臺灣時間 06:00，正在重建交易連線。"
            }}));
        }
        if self.closing.is_some_and(|at| Instant::now() >= at) {
            let _ = self.child.kill();
        }
        if let Some(light) = &mut self.light { light.heartbeat(); }
        for _ in 0..100 {
            let event = match self.events.try_recv() {
                Ok(event) => event?,
                Err(TryRecvError::Empty) => return Ok((batch, true)),
                Err(TryRecvError::Disconnected) => {
                    self.ready = false;
                    if let Ok(Some(status)) = self.child.try_wait() {
                        if !status.success() {
                            batch.push(
                                json!({"type":"status","mode":self.mode,"data":{"state":"error",
                                "message":format!("交易轉接器已結束（{status}）。")}}),
                            );
                        }
                    }
                    return Ok((batch, false));
                }
            };
            if event["mode"] != self.mode || !event["type"].is_string() {
                return Err("交易回報來源或格式不符".into());
            }
            self.journal
                .execute(
                    "INSERT INTO trade_events(profile,mode,session,event) VALUES(?1,?2,?3,?4)",
                    params![self.profile, self.mode, self.session, event.to_string()],
                )
                .map_err(|e| format!("交易回報日誌寫入失敗：{e}"))?;
            if event["type"] == "status" {
                match event["data"]["state"].as_str() {
                    Some("ready") if self.closing.is_none() => self.ready = true,
                    Some("disconnected") => {
                        self.ready = false;
                        if let Some(light) = &mut self.light { light.update("reconnecting"); }
                    }
                    Some("error") => {
                        self.ready = false;
                        if self.closing.is_none() {
                            self.retryable = false;
                            if let Some(light) = &mut self.light { light.update("offline"); }
                        }
                    }
                    _ => {}
                }
            } else if event["type"] == "recovery" && event["data"]["state"] == "complete" {
                self.recovery_ready = true;
            }
            if self.ready && self.recovery_ready {
                if let Some(light) = &mut self.light { light.update("ready"); }
            }
            batch.push(event);
        }
        Ok((batch, true))
    }
}

pub struct ManagedSession {
    root: PathBuf,
    profile: String,
    mode: String,
    foreign: bool,
    active: Option<Session>,
    retry_at: Option<Instant>,
    light: Option<Indicator>,
}

impl ManagedSession {
    pub fn start(root: &Path, profile: &str, mode: &str, foreign: bool) -> Result<Self, String> {
        let active = Session::start_market(root, profile, mode, foreign, false)?;
        Ok(Self {
            root: root.into(), profile: profile.into(), mode: mode.into(), foreign,
            active: Some(active), retry_at: None, light: None,
        })
    }

    pub fn submit(&mut self, request: Value) -> Result<(), String> {
        self.active.as_mut().ok_or("交易連線正在重建")?.submit(request)
    }

    pub fn poll(&mut self) -> Result<(Vec<Value>, bool), String> {
        if let Some(session) = &mut self.active {
            let (mut events, running) = session.poll()?;
            if running { return Ok((events, true)); }
            let retryable = session.retryable;
            let delay = if session.closing.is_some() { Duration::from_secs(1) } else { RETRY_DELAY };
            self.active = None;
            if !retryable { return Ok((events, false)); }
            let mut light = Indicator::new(&self.root, "trade", &self.profile);
            light.update("reconnecting");
            self.light = Some(light);
            self.retry_at = Some(Instant::now() + delay);
            events.push(self.reconnect_notice("交易連線重建中。請核對中斷前委託的券商回報。"));
            return Ok((events, true));
        }
        if let Some(light) = &mut self.light { light.heartbeat(); }
        if self.retry_at.is_none_or(|at| Instant::now() < at) {
            return Ok((Vec::new(), true));
        }
        match Session::start_market(&self.root, &self.profile, &self.mode, self.foreign, false) {
            Ok(session) => {
                self.active = Some(session);
                self.retry_at = None;
                self.light = None;
                Ok((vec![self.reconnect_notice("正在重新登入交易主機及回補委託回報。")], true))
            }
            Err(error) => {
                self.retry_at = Some(Instant::now() + RETRY_DELAY);
                Ok((vec![self.reconnect_notice(&error)], true))
            }
        }
    }

    fn reconnect_notice(&self, message: &str) -> Value {
        json!({"type":"status","mode":self.mode,"data":{"state":"reconnecting","message":message}})
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.input.take();
        let deadline = Instant::now() + Duration::from_millis(300);
        while Instant::now() < deadline {
            match self.child.try_wait() {
                Ok(Some(_)) => return,
                Ok(None) => std::thread::sleep(Duration::from_millis(10)),
                Err(_) => break,
            }
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[derive(GodotClass)]
#[class(base=RefCounted,init)]
struct TradeBridge {
    base: Base<RefCounted>,
    session: Option<ManagedSession>,
}

fn response(result: Result<Value, String>) -> GString {
    let value = match result {
        Ok(data) => json!({"ok":true,"data":data}),
        Err(error) => json!({"ok":false,"error":error}),
    };
    value.to_string().as_str().into()
}

fn read_trade_reports(path: &Path, filter: &Value) -> Result<Value, String> {
    if !path.is_file() {
        return Ok(json!({"rows":[],"profiles":[],"more":false}));
    }
    let conn = Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|e| e.to_string())?;
    conn.busy_timeout(Duration::from_millis(500))
        .map_err(|e| e.to_string())?;
    let exists: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='trade_events')",
            [],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    if !exists {
        return Ok(json!({"rows":[],"profiles":[],"more":false}));
    }
    let mut profiles = conn
        .prepare("SELECT DISTINCT profile FROM trade_events ORDER BY profile COLLATE NOCASE")
        .map_err(|e| e.to_string())?;
    let profiles = profiles
        .query_map([], |r| r.get::<_, String>(0))
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    let mut query=conn.prepare("WITH reports AS (
        SELECT seq,profile,mode,received_at,event,json_extract(event,'$.type') AS kind,
        CASE json_extract(event,'$.type')
            WHEN 'transport_ack' THEN json_extract(event,'$.data.received')=0
            WHEN 'order_ack' THEN trim(ltrim(trim(COALESCE(json_extract(event,'$.data.code'),'')),'0'))<>''
            WHEN 'order_report' THEN trim(ltrim(trim(COALESCE(json_extract(event,'$.data.code'),'')),'0'))<>''
            WHEN 'reply' THEN json_extract(event,'$.data.state') IN ('rejected','unknown')
            WHEN 'status' THEN json_extract(event,'$.data.state')='error'
            ELSE 0 END AS issue
        FROM trade_events
        WHERE (?1='' OR profile=?1 COLLATE NOCASE) AND (?2='' OR mode=?2) AND (?3=0 OR seq<?3)
    ) SELECT seq,profile,mode,datetime(received_at,'+8 hours'),kind,COALESCE(issue,0),event FROM reports
    WHERE kind IN ('transport_ack','order_ack','order_dispatch','order_report','fill','reply','status','recovery')
        AND (?4='' OR kind=?4 OR (?4='issue' AND issue=1) OR (?4='order_ack' AND kind='transport_ack'))
        AND (?5='' OR EXISTS(SELECT 1 FROM json_each(json_extract(event,'$.data'))
            WHERE key IN ('id','symbol','reference','cnt','execution_id','order_no','message','request_id')
              AND instr(lower(CAST(value AS TEXT)),lower(?5))>0))
    ORDER BY seq DESC LIMIT 101").map_err(|e|e.to_string())?;
    let filter_text = |name: &str| filter[name].as_str().unwrap_or_default().trim();
    let mut rows = query
        .query(params![
            filter_text("profile"),
            filter_text("mode"),
            filter["before"].as_i64().unwrap_or(0),
            filter_text("kind"),
            filter_text("search")
        ])
        .map_err(|e| e.to_string())?;
    let mut items = Vec::with_capacity(100);
    let mut more = false;
    while let Some(row) = rows.next().map_err(|e| e.to_string())? {
        if items.len() == 100 {
            more = true;
            break;
        }
        let item = || -> rusqlite::Result<Value> {
            let event: String = row.get(6)?;
            let decoded: Value = serde_json::from_str(&event).unwrap_or_default();
            Ok(
                json!({"seq":row.get::<_,i64>(0)?,"profile":row.get::<_,String>(1)?,"mode":row.get::<_,String>(2)?,
                "received":row.get::<_,Option<String>>(3)?,"kind":row.get::<_,String>(4)?,"issue":row.get::<_,bool>(5)?,"data":decoded["data"]}),
            )
        };
        items.push(item().map_err(|e| e.to_string())?);
    }
    Ok(json!({"rows":items,"profiles":profiles,"more":more}))
}

#[cfg(test)]
mod report_tests {
    use super::*;

    #[test]
    fn reports_filter_errors_and_page_without_overlapping_after_an_insert() {
        let path = workbench::root()
            .join("outputs")
            .join(crate::strategy_library::fresh_id())
            .with_extension("sqlite");
        let db = Connection::open(&path).unwrap();
        db.execute_batch("CREATE TABLE trade_events(seq INTEGER PRIMARY KEY, profile TEXT, mode TEXT, event TEXT, received_at TEXT)").unwrap();
        let insert = |profile: &str, mode: &str, event: Value| {
            db.execute("INSERT INTO trade_events(profile,mode,event,received_at) VALUES(?1,?2,?3,'2026-11-22 02:30:00')",params![profile,mode,event.to_string()]).unwrap();
        };
        for i in 1..=102 {
            insert(
                "broker",
                "read_only",
                json!({"type":"fill","data":{"symbol":"TXFZ6","execution_id":format!("F{i}")}}),
            );
        }
        insert(
            "fixture",
            "paper",
            json!({"type":"order_report","data":{"code":"00000"}}),
        );
        insert(
            "broker",
            "read_only",
            json!({"type":"transport_ack","data":{"received":false}}),
        );
        insert(
            "broker",
            "read_only",
            json!({"type":"query","data":{"action":"margin","rows":[]}}),
        );
        let filter = json!({"profile":"broker","mode":"read_only","kind":"fill","search":"txfz6"});
        let page = read_trade_reports(&path, &filter).unwrap();
        assert_eq!(page["rows"].as_array().unwrap().len(), 100);
        assert_eq!(page["more"], true);
        assert_eq!(page["rows"][0]["received"], "2026-11-22 10:30:00");
        let mut later = filter;
        later["before"] = page["rows"][99]["seq"].clone();
        insert(
            "broker",
            "read_only",
            json!({"type":"fill","data":{"symbol":"TXFZ6","execution_id":"F103"}}),
        );
        let page = read_trade_reports(&path, &later).unwrap();
        assert_eq!(page["rows"].as_array().unwrap().len(), 2);
        assert_eq!(page["more"], false);
        let issues = read_trade_reports(&path, &json!({"kind":"issue"})).unwrap();
        assert_eq!(issues["rows"].as_array().unwrap().len(), 1);
        assert_eq!(issues["rows"][0]["kind"], "transport_ack");
        let all = read_trade_reports(&path, &json!({"search":"margin"})).unwrap();
        assert!(all["rows"].as_array().unwrap().is_empty());
        drop(db);
        fs::remove_file(&path).unwrap();
    }
}

#[derive(GodotClass)]
#[class(base=RefCounted,init)]
struct TradeReportBridge {
    base: Base<RefCounted>,
    pending: Option<Receiver<String>>,
}

#[godot_api]
impl TradeReportBridge {
    #[func]
    fn start(&mut self, filter: GString) -> bool {
        if self.pending.is_some() {
            return false;
        }
        let filter = filter.to_string();
        let (send, receive) = mpsc::channel();
        self.pending = Some(receive);
        std::thread::spawn(move || {
            let result = serde_json::from_str(&filter)
                .map_err(|e| e.to_string())
                .and_then(|filter| read_trade_reports(&journal_path(&workbench::root()), &filter));
            let output = match result {
                Ok(data) => json!({"ok":true,"data":data}),
                Err(error) => json!({"ok":false,"error":error}),
            };
            let _ = send.send(output.to_string());
        });
        true
    }

    #[func]
    fn poll(&mut self) -> GString {
        let Some(pending) = &self.pending else {
            return GString::new();
        };
        match pending.try_recv() {
            Ok(result) => {
                self.pending = None;
                GString::from(&result)
            }
            Err(TryRecvError::Empty) => GString::new(),
            Err(TryRecvError::Disconnected) => {
                self.pending = None;
                response(Err("交易回報讀取中斷".into()))
            }
        }
    }
}

pub fn nikkei_quarter(now: chrono::NaiveDateTime) -> Result<Value, String> {
    use chrono::{Datelike, NaiveDate, Weekday};
    // JPX ordinary business-day calendar. Holiday trading does not advance the expiry count.
    let closures = |year| match year {
        2026 => Some("0101 0102 0103 0112 0211 0223 0320 0429 0503 0504 0505 0506 0720 0811 0921 0922 0923 1012 1103 1123 1231"),
        2027 => Some("0101 0102 0103 0111 0211 0223 0321 0322 0429 0503 0504 0505 0719 0811 0920 0923 1011 1103 1123 1231"),
        _ => None,
    };
    let dates = |year, month| -> Result<(NaiveDate, NaiveDate), String> {
        let calendar = closures(year).ok_or("JPX 行事曆尚未涵蓋所選年份，請依公告手動設定契約。")?;
        let previous = |mut day: NaiveDate| {
            loop {
                day = day.pred_opt().expect("有效的 JPX 行事曆日期");
                let code = day.format("%m%d").to_string();
                if !matches!(day.weekday(), Weekday::Sat | Weekday::Sun)
                    && !calendar.split_whitespace().any(|closed| closed == code) {
                    return day;
                }
            }
        };
        let friday = (8..=14).filter_map(|day| NaiveDate::from_ymd_opt(year, month, day))
            .find(|day| day.weekday() == Weekday::Fri).ok_or("日經季月日期無效")?;
        let last = previous(friday);
        Ok((last, previous(last)))
    };
    let mut year = now.year();
    let mut month = now.month().div_ceil(3) * 3;
    let (mut last, mut roll) = dates(year, month)?;
    if now >= roll.and_hms_opt(8, 45, 0).unwrap() {
        if month == 12 { month = 3; year += 1; } else { month += 3; }
        (last, roll) = dates(year, month)?;
    }
    Ok(json!({"maturity":format!("{year:04}{month:02}"),
        "last_trading_day":last.to_string(),"roll_date_jst":roll.to_string()}))
}

#[cfg(test)]
#[test]
fn nikkei_changes_quarters_one_business_day_early() {
    use chrono::NaiveDate;
    let at = |y,m,d,h,min| NaiveDate::from_ymd_opt(y,m,d).unwrap().and_hms_opt(h,min,0).unwrap();
    let before = nikkei_quarter(at(2026,12,9,8,44)).unwrap();
    assert_eq!(before["maturity"], "202612");
    assert_eq!(before["last_trading_day"], "2026-12-10");
    assert_eq!(before["roll_date_jst"], "2026-12-09");
    assert_eq!(nikkei_quarter(at(2026,12,9,8,45)).unwrap()["maturity"], "202703");
    assert_eq!(nikkei_quarter(at(2027,3,10,8,45)).unwrap()["maturity"], "202706");
    assert!(nikkei_quarter(at(2027,12,8,8,45)).is_err());
}

#[godot_api]
impl TradeBridge {
    #[func]
    fn nikkei_quarter(&self) -> GString {
        let now = chrono::DateTime::<chrono::Utc>::from(std::time::SystemTime::now())
            .naive_utc() + chrono::Duration::hours(9);
        response(nikkei_quarter(now))
    }
    #[func]
    fn start(&mut self, profile: GString, mode: GString) -> GString {
        self.connect_market(profile, mode, false)
    }

    #[func]
    fn start_foreign(&mut self, profile: GString, mode: GString) -> GString {
        self.connect_market(profile, mode, true)
    }

    fn connect_market(&mut self, profile: GString, mode: GString, foreign: bool) -> GString {
        if self.session.is_some() {
            return response(Err("請先關閉現有交易連線".into()));
        }
        response(
            ManagedSession::start(&workbench::root(), &profile.to_string(), &mode.to_string(), foreign).map(
                |session| {
                    self.session = Some(session);
                    json!({})
                },
            ),
        )
    }

    #[func]
    fn submit(&mut self, request: GString) -> GString {
        response(
            self.session
                .as_mut()
                .ok_or_else(|| "尚未建立交易連線".to_owned())
                .and_then(|session| {
                    serde_json::from_str(&request.to_string())
                        .map_err(|_| "交易指令格式錯誤".to_owned())
                        .and_then(|value| session.submit(value))
                })
                .map(|_| json!({})),
        )
    }

    #[func]
    fn poll(&mut self) -> GString {
        let result = self
            .session
            .as_mut()
            .map_or(Ok((Vec::new(), false)), ManagedSession::poll);
        match result {
            Ok((events, active)) => {
                if !active {
                    self.session = None;
                }
                response(Ok(json!({"events":events,"active":active})))
            }
            Err(error) => {
                self.session = None;
                response(Err(error))
            }
        }
    }

    #[func]
    fn stop(&mut self) {
        self.session = None;
    }
}

#[cfg(test)]
#[test]
fn scheduled_trade_reset_replaces_the_session_and_blocks_requests_during_restart() {
    let root = workbench::root();
    let fixture = root.join("outputs").join(crate::strategy_library::fresh_id());
    let install = fixture.join("outputs/trade-gateway");
    fs::create_dir_all(&install).unwrap();
    for file in fs::read_dir(root.join("outputs/trade-gateway")).unwrap() {
        let path = file.unwrap().path();
        if path.extension().and_then(|ext| ext.to_str()).is_some_and(|ext| matches!(ext, "dll" | "exe")) {
            fs::copy(&path, install.join(path.file_name().unwrap())).unwrap();
        }
    }
    let mut managed = ManagedSession::start(&fixture, "", "paper", false).unwrap();
    let until = Instant::now() + Duration::from_secs(10);
    let ready = |manager: &mut ManagedSession| {
        loop {
            assert!(Instant::now() < until, "模擬交易重連逾時");
            let (_, running) = manager.poll().unwrap();
            assert!(running);
            if manager.active.as_ref().is_some_and(|session| session.ready) { break; }
            std::thread::sleep(Duration::from_millis(20));
        }
    };
    ready(&mut managed);
    let before = managed.active.as_ref().unwrap().session.clone();
    let active = managed.active.as_mut().unwrap();
    active.retryable = true;
    active.reset_at = Some(api_connection::utc_now());
    assert!(managed.submit(json!({"id":"during-reset","action":"account"})).is_err());
    let (events, _) = managed.poll().unwrap();
    assert!(events.iter().any(|event| event["data"]["scheduled"] == true));
    ready(&mut managed);
    assert_ne!(before, managed.active.as_ref().unwrap().session);
    assert_eq!(database(&fixture).unwrap().query_row(
        "SELECT COUNT(*) FROM trade_commands", [], |row| row.get::<_, i64>(0)
    ).unwrap(), 0);
    drop(managed);
    let fixture = fs::canonicalize(fixture).unwrap();
    assert!(fixture.starts_with(fs::canonicalize(root.join("outputs")).unwrap()));
    fs::remove_dir_all(fixture).unwrap();
}
