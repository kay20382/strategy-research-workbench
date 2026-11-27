use crate::workbench;
use chrono::{DateTime, FixedOffset, TimeDelta, Utc};
use godot::prelude::*;
use rusqlite::{params, Connection, OpenFlags};
use serde_json::{json, Value};
use std::{path::{Path, PathBuf}, time::{Duration, Instant, SystemTime}};

const STATUS_INTERVAL: Duration = Duration::from_secs(3);
const EXPIRE_SECONDS: i64 = 12;
pub const RETRY_DELAY: Duration = Duration::from_secs(15);

pub fn utc_now() -> DateTime<Utc> {
    SystemTime::now().into()
}

pub fn next_reset(now: DateTime<Utc>) -> DateTime<Utc> {
    let taipei = FixedOffset::east_opt(8 * 3600).unwrap();
    let local = now.with_timezone(&taipei);
    let today = local.date_naive().and_hms_opt(6, 0, 0)
        .unwrap().and_local_timezone(taipei).unwrap().to_utc();
    if now < today { today } else { today + TimeDelta::days(1) }
}

fn location(root: &Path) -> PathBuf {
    std::env::var_os("STRATEGY_CONNECTION_STATUS_DB")
        .filter(|value| !value.is_empty())
        .map(|value| root.join(value))
        .unwrap_or_else(|| root.join("data/api-connection-status.sqlite"))
}

pub struct Indicator {
    path: PathBuf,
    db: Option<Connection>,
    kind: &'static str,
    label: String,
    id: String,
    state: &'static str,
    last_write: Option<Instant>,
}

impl Indicator {
    pub fn new(root: &Path, kind: &'static str, label: &str) -> Self {
        let mut out = Self {
            path: location(root), db: None, kind, label: label.into(),
            id: crate::strategy_library::fresh_id(), state: "connecting", last_write: None,
        };
        out.update("connecting");
        out
    }

    pub fn update(&mut self, state: &'static str) {
        if self.state == state && self.last_write.is_some_and(|last| last.elapsed() < STATUS_INTERVAL) {
            return;
        }
        self.state = state;
        self.last_write = Some(Instant::now());
        if self.db.is_none() {
            self.db = (|| -> rusqlite::Result<Connection> {
                let db = Connection::open(&self.path)?;
                db.busy_timeout(Duration::from_millis(50))?;
                db.execute_batch("CREATE TABLE IF NOT EXISTS connection_status(
                    id TEXT PRIMARY KEY, kind TEXT NOT NULL, label TEXT NOT NULL,
                    state TEXT NOT NULL, updated INTEGER NOT NULL)")?;
                Ok(db)
            })().ok();
        }
        if let Some(db) = &self.db {
            // Connection telemetry must never make brokerage or market data processing fail.
            let _ = db.execute(
                "INSERT INTO connection_status VALUES(?1,?2,?3,?4,?5)
                 ON CONFLICT(id) DO UPDATE SET state=excluded.state,updated=excluded.updated",
                params![self.id,self.kind,self.label,self.state,utc_now().timestamp()],
            );
        }
    }

    pub fn heartbeat(&mut self) { self.update(self.state); }
}

impl Drop for Indicator {
    fn drop(&mut self) {
        if self.state == "offline" { return; }
        if let Some(db) = &self.db {
            let _ = db.execute("DELETE FROM connection_status WHERE id=?1", [&self.id]);
        }
    }
}

fn snapshot(root: &Path) -> rusqlite::Result<Value> {
    let path = location(root);
    if !path.is_file() {
        return Ok(json!([]));
    }
    let db = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    db.busy_timeout(Duration::from_millis(15))?;
    let mut query = db.prepare("SELECT kind,label,state FROM connection_status
        WHERE updated>=?1 ORDER BY kind,label,updated DESC,id")?;
    let rows = query.query_map([utc_now().timestamp() - EXPIRE_SECONDS], |row| {
        Ok(json!({"kind": row.get::<_,String>(0)?, "label": row.get::<_,String>(1)?,
            "state": row.get::<_,String>(2)?}))
    })?.collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(json!(rows))
}

#[derive(GodotClass)]
#[class(base=RefCounted,init)]
struct ApiConnectionBridge {
    base: Base<RefCounted>,
}

#[godot_api]
impl ApiConnectionBridge {
    #[func]
    fn snapshot(&self) -> GString {
        snapshot(&workbench::root()).unwrap_or(Value::Null).to_string().as_str().into()
    }
}

#[cfg(test)]
#[test]
fn reset_uses_taipei_06_and_advances_to_the_next_day_on_the_boundary() {
    let at = |s: &str| DateTime::parse_from_rfc3339(s).unwrap().to_utc();
    assert_eq!(next_reset(at("2026-11-24T21:59:59Z")), at("2026-11-24T22:00:00Z"));
    assert_eq!(next_reset(at("2026-11-24T22:00:00Z")), at("2026-11-25T22:00:00Z"));
}
