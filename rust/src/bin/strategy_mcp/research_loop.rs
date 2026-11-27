use super::{parse, tool, Candidate, Server};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    fs,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

pub(super) fn tools() -> Vec<Value> {
    let id = json!({"type":"string","description":"loop_start 回傳的 loop_id"});
    let round = json!({"type":"integer","minimum":1});
    let score = json!({"type":"number","minimum":0,"maximum":100});
    vec![
        tool(
            "loop_start",
            "建立可續跑的研究迴圈。先讀原版報表，按使用者準則評分，再登錄基準。",
            json!({"baseline_run_id":{"type":"string"},"goal":{"type":"string"},"rubric":{"type":"string"},
                "max_rounds":{"type":"integer","minimum":1,"maximum":100},"baseline_score":score,
                "baseline_review":{"type":"string"}}),
            &[
                "baseline_run_id",
                "goal",
                "rubric",
                "max_rounds",
                "baseline_score",
                "baseline_review",
            ],
            false,
        ),
        tool(
            "loop_status",
            "取得研究目標、評分準則、基準、歷次候選、目前最佳版本與下一步。",
            json!({"loop_id":id,"include_failed_source":{"type":"boolean","default":false}}),
            &["loop_id"],
            true,
        ),
        tool(
            "loop_submit",
            "提交下一輪完整 PL 和修改假設。自動比對原版行情與設定並保存回測或拒絕原因。",
            json!({"loop_id":id,"round":round,"source":{"type":"string"},"hypothesis":{"type":"string"}}),
            &["loop_id", "round", "source", "hypothesis"],
            false,
        ),
        tool(
            "loop_score",
            "將助理讀完候選報表後給的分數與理由記入該輪。較高分數優先，同分保留較早版本。",
            json!({"loop_id":id,"round":round,"score":score,"review":{"type":"string"}}),
            &["loop_id", "round", "score", "review"],
            false,
        ),
        tool(
            "loop_finish",
            "記錄停止原因，結束研究迴圈並傳回最佳版本。",
            json!({"loop_id":id,"reason":{"type":"string"}}),
            &["loop_id", "reason"],
            false,
        ),
    ]
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Start {
    baseline_run_id: String,
    goal: String,
    rubric: String,
    max_rounds: usize,
    baseline_score: f64,
    baseline_review: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Status {
    loop_id: String,
    #[serde(default)]
    include_failed_source: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Submit {
    loop_id: String,
    round: usize,
    source: String,
    hypothesis: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Score {
    loop_id: String,
    round: usize,
    score: f64,
    review: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Finish {
    loop_id: String,
    reason: String,
}

#[derive(Serialize, Deserialize)]
struct Assessment {
    score: f64,
    review: String,
}

#[derive(Serialize, Deserialize)]
struct Round {
    round: usize,
    hypothesis: String,
    source_sha256: String,
    run_id: Option<String>,
    summary: Option<Value>,
    delta: Option<Value>,
    error: Option<String>,
    assessment: Option<Assessment>,
}

#[derive(Serialize, Deserialize)]
struct ResearchLoop {
    loop_id: String,
    baseline_run_id: String,
    baseline: Assessment,
    goal: String,
    rubric: String,
    max_rounds: usize,
    rounds: Vec<Round>,
    finish_reason: Option<String>,
    #[serde(default)]
    last_failed_source: Option<String>,
}

impl ResearchLoop {
    fn status(&self, server: &Server) -> Value {
        let baseline_error = server.validate_research_run(&self.baseline_run_id).err();
        let mut best = if baseline_error.is_none() {
            json!({"run_id":self.baseline_run_id,"score":self.baseline.score,"round":0})
        } else { Value::Null };
        let mut highest = self.baseline.score;
        let mut rejected_runs = Vec::new();
        let mut awaiting_score = None;
        for round in &self.rounds {
            let Some(id) = round.run_id.as_deref() else { continue };
            if let Err(error) = server.validate_research_run(id) {
                rejected_runs.push(json!({"round":round.round,"run_id":id,"error":error}));
                continue;
            }
            if let Some(assessment) = &round.assessment {
                if assessment.score > highest {
                    highest = assessment.score;
                    best = json!({"run_id":round.run_id,"score":highest,"round":round.round});
                }
            } else if Some(round.round) == self.rounds.last().map(|last| last.round) {
                awaiting_score = Some(round.round);
            }
        }
        let next_action = if self.finish_reason.is_some() {
            "finished"
        } else if baseline_error.is_some() {
            "restart"
        } else if awaiting_score.is_some() {
            "score"
        } else if self.rounds.len() >= self.max_rounds {
            "finish"
        } else {
            "submit"
        };
        let mut state = json!(self);
        if let Some(fields) = state.as_object_mut() {
            fields.remove("last_failed_source");
        }
        if baseline_error.is_some() { best = Value::Null; }
        json!({"loop":state,"best":best,"next_action":next_action,"baseline_validation_error":baseline_error,
            "next_round":if next_action=="submit" {Some(self.rounds.len()+1)} else {None},
            "awaiting_score":if next_action=="score" {awaiting_score} else {None},"rejected_runs":rejected_runs})
    }
}

fn assessment(score: f64, review: String) -> Result<Assessment, String> {
    if !(0.0..=100.0).contains(&score) || review.trim().is_empty() {
        return Err("分數須介於 0 至 100，並附上評分理由".into());
    }
    Ok(Assessment { score, review })
}

fn sql<T>(value: rusqlite::Result<T>) -> Result<T, String> {
    value.map_err(|err| err.to_string())
}

impl Server {
    fn loop_db(&self, id: &str, create: bool) -> Result<Connection, String> {
        if !super::valid_id(id, "loop-") {
            return Err("loop_id 格式錯誤".into());
        }
        let dir = std::env::var_os("REA_MCP_LOOP_DIRECTORY")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| self.root.join("outputs/strategy-research-loops"));
        let path = dir.join(format!("{id}.sqlite"));
        let conn = if create {
            fs::create_dir_all(dir).map_err(|err| err.to_string())?;
            sql(Connection::open(path))?
        } else {
            sql(Connection::open_with_flags(
                path,
                rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE,
            ))?
        };
        sql(conn.busy_timeout(Duration::from_secs(5)))?;
        if create {
            sql(conn.execute_batch("PRAGMA journal_mode=WAL; CREATE TABLE IF NOT EXISTS research_loops(id TEXT PRIMARY KEY, body TEXT NOT NULL);"))?;
        }
        Ok(conn)
    }

    fn load_loop(conn: &Connection, id: &str) -> Result<ResearchLoop, String> {
        let data = sql(conn
            .query_row("SELECT body FROM research_loops WHERE id=?1", [id], |row| {
                row.get::<_, String>(0)
            })
            .optional())?
        .ok_or("找不到此研究迴圈")?;
        serde_json::from_str(&data).map_err(|err| err.to_string())
    }

    fn edit_loop(
        &self,
        id: &str,
        edit: impl FnOnce(&mut ResearchLoop) -> Result<(), String>,
    ) -> Result<Value, String> {
        let mut conn = self.loop_db(id, false)?;
        let tx = sql(conn.transaction_with_behavior(TransactionBehavior::Immediate))?;
        let mut state = Self::load_loop(&tx, id)?;
        self.validate_research_run(&state.baseline_run_id)?;
        edit(&mut state)?;
        self.check_cancelled()?;
        let body = serde_json::to_string(&state).map_err(|err| err.to_string())?;
        sql(tx.execute(
            "UPDATE research_loops SET body=?1 WHERE id=?2",
            params![body, id],
        ))?;
        sql(tx.commit())?;
        Ok(state.status(self))
    }

    pub(super) fn loop_call(&self, name: &str, args: Value) -> Result<Value, String> {
        match name {
            "loop_start" => {
                let args: Start = parse(args)?;
                self.baseline(&args.baseline_run_id)?;
                if !(1..=100).contains(&args.max_rounds)
                    || args.goal.trim().is_empty()
                    || args.rubric.trim().is_empty()
                {
                    return Err("請提供研究目標、評分準則及 1 至 100 輪的上限".into());
                }
                let now = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map_err(|err| err.to_string())?;
                let state = ResearchLoop {
                    loop_id: format!("loop-{}-{}", now.as_nanos(), std::process::id()),
                    baseline_run_id: args.baseline_run_id,
                    baseline: assessment(args.baseline_score, args.baseline_review)?,
                    goal: args.goal,
                    rubric: args.rubric,
                    max_rounds: args.max_rounds,
                    rounds: vec![],
                    finish_reason: None,
                    last_failed_source: None,
                };
                let body = serde_json::to_string(&state).map_err(|err| err.to_string())?;
                sql(self.loop_db(&state.loop_id, true)?.execute(
                    "INSERT INTO research_loops(id, body) VALUES (?1,?2)",
                    params![state.loop_id, body],
                ))?;
                Ok(state.status(self))
            }
            "loop_status" => {
                let args: Status = parse(args)?;
                let state = Self::load_loop(&self.loop_db(&args.loop_id, false)?, &args.loop_id)?;
                let mut status = state.status(self);
                if args.include_failed_source {
                    status["last_failed_source"] = json!(state.last_failed_source);
                }
                Ok(status)
            }
            "loop_submit" => {
                let args: Submit = parse(args)?;
                if args.hypothesis.trim().is_empty()
                    || args.source.trim().is_empty()
                    || args.source.len() > 100_000
                {
                    return Err("請提供修改假設與完整 PL，原始碼上限為 100000 位元組".into());
                }
                self.edit_loop(&args.loop_id, |state| {
                    let sha = format!("{:x}", Sha256::digest(args.source.trim().as_bytes()));
                    if let Some(prior) = args
                        .round
                        .checked_sub(1)
                        .and_then(|index| state.rounds.get(index))
                    {
                        if prior.source_sha256 == sha && prior.hypothesis == args.hypothesis {
                            return Ok(());
                        }
                        return Err("該輪已有不同的提交".into());
                    }
                    if state.status(self)["next_action"] != "submit"
                        || args.round != state.rounds.len() + 1
                    {
                        return Err("目前無法提交此輪，請依 loop_status 的下一步處理".into());
                    }
                    if state.rounds.iter().any(|round| round.source_sha256 == sha) {
                        return Err("相同程式碼已在此迴圈提交".into());
                    }
                    let mut round = Round {
                        round: args.round,
                        hypothesis: args.hypothesis,
                        source_sha256: sha,
                        run_id: None,
                        summary: None,
                        delta: None,
                        error: None,
                        assessment: None,
                    };
                    match self.compare(Candidate {
                        baseline_run_id: state.baseline_run_id.clone(),
                        source: args.source.clone(),
                    }) {
                        Ok(result) => {
                            state.last_failed_source = None;
                            round.run_id =
                                result["candidate"]["run_id"].as_str().map(str::to_owned);
                            round.summary = Some(result["candidate"]["summary"].clone());
                            round.delta = Some(result["delta"].clone());
                        }
                        Err(err) => {
                            round.error = Some(err);
                            state.last_failed_source = Some(args.source);
                        }
                    }
                    state.rounds.push(round);
                    Ok(())
                })
            }
            "loop_score" => {
                let args: Score = parse(args)?;
                let assessment = assessment(args.score, args.review)?;
                self.edit_loop(&args.loop_id, |state| {
                    let round = args
                        .round
                        .checked_sub(1)
                        .and_then(|index| state.rounds.get_mut(index))
                        .ok_or("找不到該輪")?;
                    self.validate_research_run(round.run_id.as_deref().ok_or("只有通過成交驗收的回測可評分")?)?;
                    if let Some(old) = &round.assessment {
                        if old.score == assessment.score && old.review == assessment.review {
                            return Ok(());
                        }
                        return Err("該輪已有不同的評分".into());
                    }
                    if state.finish_reason.is_some() || round.run_id.is_none() {
                        return Err("只有尚在進行中的成功回測可評分".into());
                    }
                    round.assessment = Some(assessment);
                    Ok(())
                })
            }
            "loop_finish" => {
                let args: Finish = parse(args)?;
                if args.reason.trim().is_empty() {
                    return Err("請填寫停止原因".into());
                }
                self.edit_loop(&args.loop_id, |state| {
                    if let Some(old) = &state.finish_reason {
                        if old == &args.reason {
                            return Ok(());
                        }
                        return Err("此迴圈已結束".into());
                    }
                    if state.status(self)["next_action"] == "score" {
                        return Err("請先完成最後一輪的評分".into());
                    }
                    state.finish_reason = Some(args.reason);
                    Ok(())
                })
            }
            _ => Err(format!("找不到 MCP 工具：{name}")),
        }
    }
}
