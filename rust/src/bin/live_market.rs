use recharts_core::{api_connection::{self, Indicator, RETRY_DELAY}, api_profiles, live_market::{recoverable_trade_rejection, Store}, workbench};
use std::{
    io::{self, BufRead, BufReader},
    path::Path,
    process::{Command, Stdio},
    sync::mpsc,
    time::{Duration, Instant},
};

fn consume(store: &mut Store, input: impl BufRead, connected: bool) -> Result<(), String> {
    for line in input.lines() {
        let line = line.map_err(|e| e.to_string())?;
        if let Err(reason) = store.ingest(&line) {
            if !connected || !recoverable_trade_rejection(&reason) {
                return Err(reason);
            }
            eprintln!("成交已記入拒收紀錄，持續接收行情：{reason}");
        }
    }
    Ok(())
}

fn configure_gateway(command: &mut Command, id: &str) -> Result<String, String> {
    let (profile, credentials) =
        api_profiles::load(&api_profiles::location(&workbench::root()), id)?;
    if profile.provider != "kgi_quotecom" || profile.purpose != "quote" {
        return Err("此行情轉接器需要 kgi_quotecom 行情設定".into());
    }
    let connection = if profile.connection.trim().is_empty() {
        profile.id
    } else {
        profile.connection
    };
    let settings = [
        (
            "QUOTECOM_CONNECTION",
            "連線字串前綴",
            Some(connection.clone()),
        ),
        ("QUOTECOM_HOST", "主機", Some(profile.host)),
        (
            "QUOTECOM_PORT",
            "連接埠",
            profile.port.map(|p| p.to_string()),
        ),
        ("QUOTECOM_SOURCE_ID", "來源 ID", Some(profile.source_id)),
        ("QUOTECOM_TOKEN", "權杖", credentials.token),
        ("QUOTECOM_ID", "帳號", credentials.account),
        ("QUOTECOM_PASSWORD", "密碼", credentials.password),
    ];
    for (key, label, value) in settings {
        let value = value
            .filter(|s| !s.trim().is_empty())
            .ok_or_else(|| format!("API 設定缺少{label}"))?;
        command.env(key, value);
    }
    Ok(connection)
}

fn jpx_contract_now() -> Result<String, String> {
    let now = api_connection::utc_now().naive_utc() + chrono::TimeDelta::hours(9);
    let current = recharts_core::trading::nikkei_quarter(now)?;
    current["maturity"].as_str().map(str::to_owned).ok_or("無法辨識日經連續行情來源".into())
}

fn run() -> Result<(), String> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let [path, zone, policy, rest @ ..] = args.as_slice() else {
        return Err("用法：live_market 資料庫 時區 taifex|minute [--reconnect] [--profile 設定名稱] [--gateway 程式 [參數…]]".into());
    };
    let taifex = match policy.as_str() {
        "taifex" => true,
        "minute" => false,
        _ => return Err("收棒模式須為 taifex 或 minute".into()),
    };
    if rest.is_empty() {
        let mut store = Store::open(&workbench::root(), Path::new(path), zone, taifex)?;
        return consume(&mut store, io::stdin().lock(), false);
    }
    let (reconnect, rest) = match rest {
        [flag, tail @ ..] if flag == "--reconnect" => (true, tail),
        _ => (false, rest),
    };
    let (profile, rest) = match rest {
        [flag, id, tail @ ..] if flag == "--profile" => (Some(id), tail),
        _ => (None, rest),
    };
    let [flag, program, gateway_args @ ..] = rest else {
        return Err("缺少行情轉接程式".into());
    };
    if flag != "--gateway" {
        return Err("請以 --gateway 指定行情轉接程式".into());
    }
    let root = workbench::root();
    let automatic_jpx = gateway_args == ["jpx225m"];
    if automatic_jpx && !reconnect {
        return Err("日經連續行情須開啟自動重連。".into());
    }
    let mut probe = Command::new(program);
    let label = match profile {
        Some(id) => configure_gateway(&mut probe, id)?,
        None => std::env::var("QUOTECOM_CONNECTION").unwrap_or_else(|_| "QuoteCom".into()),
    };
    let market = if gateway_args.first().is_some_and(|arg| arg == "jpx225m") { "日經" } else { "臺期所" };
    let mut light = Indicator::new(&root, "quote", &format!("{market} · {label}"));
    let mut store = Store::open(&root, Path::new(path), zone, taifex)?;
    loop {
        light.update("connecting");
        let mut command = Command::new(program);
        if let Some(id) = profile { configure_gateway(&mut command, id)?; }
        command.args(gateway_args).stdout(Stdio::piped());
        let jpx_contract = automatic_jpx.then(jpx_contract_now).transpose()?;
        if let Some(contract) = &jpx_contract { command.arg(contract); }
        if reconnect { command.env("QUOTECOM_SUPERVISED", "1").stdin(Stdio::piped()); }
        let mut child = command.spawn().map_err(|e| e.to_string())?;
        let reset = api_connection::next_reset(api_connection::utc_now());
        let stdout = child.stdout.take().expect("行情程式已設定 stdout 管線");
        let (sender, receiver) = mpsc::sync_channel(128);
        let reader = std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                if sender.send(line).is_err() { break; }
            }
        });
        let mut close_deadline = None;
        let mut next_schedule_check = Instant::now();
        let mut schedule_error = None;
        let result = loop {
            let mut roll = false;
            if jpx_contract.is_some() && close_deadline.is_none() && Instant::now() >= next_schedule_check {
                next_schedule_check = Instant::now() + Duration::from_secs(1);
                match jpx_contract_now() {
                    Ok(current) => roll = jpx_contract.as_ref() != Some(&current),
                    Err(error) => { schedule_error = Some(error); roll = true; }
                }
            }
            if reconnect && close_deadline.is_none() && (roll || api_connection::utc_now() >= reset) {
                eprintln!("{}", if roll { "日經連續行情切換來源。" } else { "臺灣時間 06:00，重新登入行情並訂閱。" });
                light.update("reconnecting");
                child.stdin.take();
                close_deadline = Some(Instant::now() + Duration::from_secs(5));
            }
            if close_deadline.is_some_and(|deadline| Instant::now() >= deadline) {
                let _ = child.kill();
            }
            light.heartbeat();
            match receiver.recv_timeout(Duration::from_millis(250)) {
                Ok(Ok(line)) => {
                    if close_deadline.is_none() {
                        if let Ok(event) = serde_json::from_str::<serde_json::Value>(&line) {
                            match event["state"].as_str() {
                                Some("QUOTE_READY") => light.update("ready"),
                                Some("DISCONNECTED" | "CONNECT_FAIL") => light.update("reconnecting"),
                                Some("LOGIN_FAIL" | "LOGIN_UNKNOW") => light.update("offline"),
                                _ => {}
                            }
                        }
                    }
                    if let Err(reason) = store.ingest(&line) {
                        if !recoverable_trade_rejection(&reason) { break Err(reason); }
                        eprintln!("成交已記入拒收紀錄，持續接收行情：{reason}");
                    }
                }
                Ok(Err(error)) => break Err(error.to_string()),
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => break Ok(()),
            }
        };
        drop(receiver);
        if result.is_err() { let _ = child.kill(); }
        let status = child.wait().map_err(|e| e.to_string())?;
        let _ = reader.join();
        result?;
        if let Some(error) = schedule_error { light.update("offline"); return Err(error); }
        let delay = if close_deadline.is_some() {
            Duration::from_secs(1)
        } else if status.success() {
            break;
        } else if reconnect && status.code() == Some(2) {
            eprintln!("行情已斷線，15 秒後重新連線與訂閱。");
            RETRY_DELAY
        } else {
            light.update("offline");
            return Err(format!("行情轉接程式已停止（{status}）"));
        };
        light.update("reconnecting");
        let until = Instant::now() + delay;
        while Instant::now() < until {
            light.heartbeat();
            std::thread::sleep(Duration::from_millis(250));
        }
    }
    Ok(())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
