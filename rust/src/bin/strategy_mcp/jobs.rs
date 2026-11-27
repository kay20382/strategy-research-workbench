use super::{error, tool_result, Server};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    io::{self, Write},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread::{self, JoinHandle},
    time::Instant,
};

const BACKGROUND: &[&str] = &[
    "backtest_run",
    "robustness_run",
    "optimization_compare",
    "report_read",
    "model_update_report",
    "loop_submit",
];
const MAX_ACTIVE: usize = 4;
const MAX_FINISHED: usize = 32;

pub(super) fn tools() -> [Value; 3] {
    let id = json!({"job_id":{"type":"string"}});
    [
        super::tool(
            "job_start",
            "提交背景工作並立即取得 job_id。",
            json!({"tool":{"type":"string","enum":BACKGROUND},"arguments":{"type":"object"}}),
            &["tool", "arguments"],
            false,
        ),
        super::tool(
            "job_status",
            "查詢本連線背景工作的階段、耗時及結果。",
            id.clone(),
            &["job_id"],
            true,
        ),
        super::tool(
            "job_cancel",
            "要求取消本連線的背景工作，透過 job_status 查詢最終狀態。",
            id,
            &["job_id"],
            false,
        ),
    ]
}

#[derive(Clone, Default)]
pub(super) struct Output(Arc<Mutex<()>>);

impl Output {
    pub(super) fn write(&self, message: &Value) -> io::Result<()> {
        let _guard = self.0.lock().unwrap();
        let mut stdout = io::stdout().lock();
        writeln!(stdout, "{message}")?;
        stdout.flush()
    }
}

struct State {
    message: String,
    progress: u64,
    cancelled_request: bool,
    result: Option<Result<Value, String>>,
    cancelled: bool,
    finished_ms: Option<u64>,
}

pub(super) struct Job {
    pub(super) stop: AtomicBool,
    id: String,
    tool: String,
    started: Instant,
    request_id: Option<Value>,
    token: Option<Value>,
    output: Output,
    state: Mutex<State>,
}

impl Job {
    fn status(&self) -> Value {
        let state = self.state.lock().unwrap();
        let status = match &state.result {
            Some(_) if state.cancelled => "cancelled",
            Some(Ok(_)) => "completed",
            Some(Err(_)) => "failed",
            None if self.stop.load(Ordering::Acquire) => "cancelling",
            None => "running",
        };
        let mut view = json!({"job_id":self.id,"tool":self.tool,"status":status,
            "stage":state.message,"progress":state.progress,"elapsed_ms":state.finished_ms
                .unwrap_or_else(|| self.started.elapsed().as_millis() as u64)});
        match &state.result {
            Some(Ok(value)) => view["result"] = value.clone(),
            Some(Err(reason)) => view["error"] = json!(reason),
            None => {}
        }
        view
    }

    fn cancel(&self, notification: bool) {
        let mut state = self.state.lock().unwrap();
        if state.result.is_none() {
            state.cancelled_request |= notification;
            self.stop.store(true, Ordering::Release);
        }
    }

    fn report(&self, line: &str) {
        let mut state = self.state.lock().unwrap();
        if self.stop.load(Ordering::Acquire) || state.result.is_some() {
            return;
        }
        let line = line
            .trim()
            .lines()
            .find(|line| !line.starts_with('{'))
            .unwrap_or("");
        if line.is_empty() {
            return;
        }
        state.message = line.chars().take(240).collect();
        state.progress += 1;
        if let Some(token) = &self.token {
            let _ = self
                .output
                .write(&json!({"jsonrpc":"2.0","method":"notifications/progress",
                "params":{"progressToken":token,"progress":state.progress,
                "message":format!("{} · {}",self.id,state.message)}}));
        }
    }

    fn finish(&self, result: Result<Value, String>, protocol: Option<&str>) {
        let mut state = self.state.lock().unwrap();
        state.cancelled = result.is_err()
            && (state.cancelled_request || self.stop.load(Ordering::Acquire));
        let result = if state.cancelled {
            Err("工作已取消".into())
        } else {
            result
        };
        if let Some(id) = self
            .request_id
            .as_ref()
            .filter(|_| !state.cancelled_request)
        {
            let mut response = tool_result(result.clone(), protocol);
            response["_meta"] = json!({"job_id":self.id});
            let _ = self
                .output
                .write(&json!({"jsonrpc":"2.0","id":id,"result":response}));
        }
        state.message = if state.cancelled {
            "工作已取消"
        } else if result.is_ok() {
            "已完成"
        } else {
            "工作失敗"
        }
        .into();
        state.finished_ms = Some(self.started.elapsed().as_millis() as u64);
        state.result = Some(result);
    }
}

#[derive(Default)]
pub(super) struct Jobs {
    next: u64,
    entries: BTreeMap<u64, (Arc<Job>, JoinHandle<()>)>,
}

impl Jobs {
    pub(super) fn dispatch(
        &mut self,
        server: &Server,
        msg: &Value,
        output: &Output,
    ) -> io::Result<bool> {
        if msg["jsonrpc"] != "2.0" || !server.ready {
            return Ok(false);
        }
        let parameters = &msg["params"];
        if msg["method"] == "notifications/cancelled" && msg.get("id").is_none() {
            for (job, _) in self.entries.values() {
                if job.request_id.as_ref() == parameters.get("requestId")
                    && job.request_id.is_some()
                {
                    job.cancel(true);
                }
            }
            return Ok(true);
        }
        let Some(id) = msg
            .get("id")
            .filter(|id| id.is_string() || id.is_i64() || id.is_u64())
        else {
            return Ok(false);
        };
        if msg["method"] != "tools/call" {
            return Ok(false);
        }
        let Some(name) = parameters["name"].as_str() else {
            return Ok(false);
        };
        let args = parameters
            .get("arguments")
            .cloned()
            .unwrap_or_else(|| json!({}));
        if !args.is_object() {
            return Ok(false);
        }
        let reply = |result| {
            output.write(
                &json!({"jsonrpc":"2.0","id":id,"result":tool_result(result,server.protocol)}),
            )
        };
        match name {
            "job_status" | "job_cancel" => {
                let job = args["job_id"]
                    .as_str()
                    .and_then(|id| id.strip_prefix("job-"))
                    .and_then(|id| id.parse::<u64>().ok())
                    .and_then(|number| self.entries.get(&number));
                reply(
                    job.ok_or_else(|| "此 MCP 連線找不到該工作".to_owned())
                        .map(|(job, _)| {
                            if name == "job_cancel" {
                                job.cancel(false);
                            }
                            job.status()
                        }),
                )?;
            }
            "job_start" => {
                let args = serde_json::from_value::<Start>(args).map_err(|err| err.to_string());
                reply(
                    args.and_then(|args| {
                        self.start(server, &args.tool, args.arguments, None, None, output)
                    })
                    .map(|job| job.status()),
                )?;
            }
            name if BACKGROUND.contains(&name) => {
                if self.entries.values().any(|(job, handle)| {
                    job.request_id.as_ref() == Some(id) && !handle.is_finished()
                }) {
                    output.write(&error(id.clone(), -32600, "請求編號仍在使用"))?;
                    return Ok(true);
                }
                let token = parameters["_meta"]["progressToken"].clone();
                let token =
                    (token.is_string() || token.is_i64() || token.is_u64()).then_some(token);
                if let Err(reason) = self.start(server, name, args, Some(id.clone()), token, output)
                {
                    reply(Err(reason))?;
                }
            }
            _ => return Ok(false),
        }
        Ok(true)
    }

    fn start(
        &mut self,
        server: &Server,
        name: &str,
        args: Value,
        request_id: Option<Value>,
        token: Option<Value>,
        output: &Output,
    ) -> Result<Arc<Job>, String> {
        if !BACKGROUND.contains(&name) || !args.is_object() {
            return Err("請指定支援背景執行的工具及其參數物件".into());
        }
        let finished: Vec<_> = self
            .entries
            .iter()
            .filter(|(_, (_, worker))| worker.is_finished())
            .map(|(&id, _)| id)
            .collect();
        let active = self.entries.len() - finished.len();
        if active >= MAX_ACTIVE {
            return Err("背景工作已滿，請等候目前工作結束".into());
        }
        let excess = (finished.len() + 1).saturating_sub(MAX_FINISHED);
        for id in finished.into_iter().take(excess) {
            if let Some((_, worker)) = self.entries.remove(&id) {
                let _ = worker.join();
            }
        }
        self.next += 1;
        let job = Arc::new(Job {
            id: format!("job-{}", self.next),
            tool: name.to_owned(),
            started: Instant::now(),
            stop: AtomicBool::new(false),
            request_id,
            token,
            output: output.clone(),
            state: Mutex::new(State {
                message: "已受理".into(),
                progress: 0,
                cancelled_request: false,
                result: None,
                cancelled: false,
                finished_ms: None,
            }),
        });
        let mut worker_server = server.clone();
        worker_server.job = Some(job.clone());
        let worker_job = job.clone();
        let worker = thread::Builder::new()
            .name(job.id.clone())
            .spawn(move || {
                worker_job.report("已開始背景工作");
                let result = std::panic::catch_unwind(|| {
                    worker_server
                        .check_cancelled()
                        .and_then(|()| worker_server.call(&worker_job.tool, args))
                })
                .unwrap_or_else(|_| Err("背景工作異常中止".into()));
                worker_job.finish(result, worker_server.protocol);
            })
            .map_err(|e| e.to_string())?;
        self.entries.insert(self.next, (job.clone(), worker));
        Ok(job)
    }
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Start {
    tool: String,
    arguments: Value,
}

impl Drop for Jobs {
    fn drop(&mut self) {
        for (job, _) in self.entries.values() {
            job.cancel(true);
        }
        for (_, (_, worker)) in std::mem::take(&mut self.entries) {
            let _ = worker.join();
        }
    }
}

impl Server {
    pub(super) fn check_cancelled(&self) -> Result<(), String> {
        if self
            .job
            .as_ref()
            .is_some_and(|job| job.stop.load(Ordering::Acquire))
        {
            return Err("工作已取消".into());
        }
        Ok(())
    }

    pub(super) fn progress(&self, message: &str) {
        eprintln!("{message}");
        if let Some(job) = &self.job {
            job.report(message);
        }
    }
}
