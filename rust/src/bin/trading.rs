use recharts_core::{trading::ManagedSession, workbench};
use serde_json::{json, Value};
use std::{
    io::{self, BufRead, Write},
    sync::mpsc,
    time::Duration,
};

fn run() -> Result<(), String> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let (mode, profile) = match args.as_slice() {
        [mode] if mode == "paper" => (mode.as_str(), ""),
        [mode, profile] if mode == "read_only" || mode == "live" => {
            (mode.as_str(), profile.as_str())
        }
        _ => return Err("用法：trading paper 或 trading read_only|live 設定名稱".into()),
    };
    let mut session = ManagedSession::start(&workbench::root(), profile, mode, false)?;
    let (tx, rx) = mpsc::sync_channel(64);
    std::thread::spawn(move || {
        for line in io::stdin().lock().lines() {
            match line {
                Ok(line) if tx.send(line.clone()).is_ok() => {}
                _ => break,
            }
        }
    });
    let mut output = io::stdout().lock();
    loop {
        let (events, active) = session.poll()?;
        for event in events {
            writeln!(output, "{event}").map_err(|e| e.to_string())?;
        }
        output.flush().map_err(|e| e.to_string())?;
        if !active {
            return Ok(());
        }
        match rx.recv_timeout(Duration::from_millis(25)) {
            Ok(line) => {
                let value: Value = match serde_json::from_str(&line) {
                    Ok(value) => value,
                    Err(_) => {
                        writeln!(output,"{}",json!({"type":"reply","mode":mode,"data":{"state":"rejected","message":"交易指令格式錯誤"}})).map_err(|e|e.to_string())?;
                        continue;
                    }
                };
                let id = value["id"].as_str().unwrap_or("").to_owned();
                if let Err(error) = session.submit(value) {
                    writeln!(output,"{}",json!({"type":"reply","mode":mode,"data":{"id":id,"state":"rejected","message":error}}))
                        .map_err(|e|e.to_string())?;
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => return Ok(()),
        }
    }
}

fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
