//! Broker-independent trade ingestion. One transaction per input event.
use crate::{canonical_market, database, tick_bars, tick_store};
use chrono::{DateTime, NaiveDateTime};
use rusqlite::{params, Connection, OptionalExtension, Transaction, TransactionBehavior};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::path::Path;

const SEALED_MINUTE: &str = "成交落在已確認的分鐘，需核對後修訂";
pub(crate) const STOCK_POLICY: &str = "taifex_stock";
const OUT_OF_SESSION: &str = "成交時間超出設定的交易時段";

/// 這些事件已寫入拒收紀錄，可繼續接收之後的即時成交。
pub fn recoverable_trade_rejection(reason: &str) -> bool {
    matches!(reason, SEALED_MINUTE | OUT_OF_SESSION | tick_store::LIVE_OUT_OF_ORDER)
}

#[derive(Deserialize)]
pub struct Envelope {
    pub protocol_version: u32,
    pub provider_id: String,
    pub connection_id: String,
    pub connection_epoch: String,
    pub event_id: i64,
    #[serde(flatten)]
    pub event: Event,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    Trade {
        instrument: String,
        event_time: String,
        price: String,
        quantity: String,
        #[serde(default)]
        foreign_trade_key: Option<String>,
    },
    Watermark {
        event_time: String,
    },
    Status {
        state: String,
    },
}

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS live_events(
 id INTEGER PRIMARY KEY, provider TEXT NOT NULL, connection TEXT NOT NULL, epoch TEXT NOT NULL,
 event_id INTEGER NOT NULL, body TEXT NOT NULL, UNIQUE(provider,connection,epoch,event_id));
CREATE TABLE IF NOT EXISTS live_rejections(id INTEGER PRIMARY KEY, body TEXT NOT NULL, reason TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS live_foreign_trade_keys(
 provider TEXT NOT NULL, connection TEXT NOT NULL, symbol TEXT NOT NULL,
 trade_key TEXT NOT NULL, price INTEGER NOT NULL, volume INTEGER NOT NULL,
 PRIMARY KEY(provider,connection,symbol,trade_key));
CREATE TABLE IF NOT EXISTS live_watermarks(
 provider TEXT NOT NULL, connection TEXT NOT NULL, timezone TEXT NOT NULL,
 session_policy TEXT NOT NULL, cutoff TEXT NOT NULL,
 PRIMARY KEY(provider,connection,timezone,session_policy));
CREATE TABLE IF NOT EXISTS live_sources(
 symbol TEXT PRIMARY KEY COLLATE NOCASE, provider TEXT NOT NULL, connection TEXT NOT NULL,
 timezone TEXT NOT NULL, session_policy TEXT NOT NULL,
 tick_id INTEGER NOT NULL REFERENCES datasets(id), minute_id INTEGER NOT NULL REFERENCES datasets(id));
CREATE TABLE IF NOT EXISTS live_pending(
 event_id INTEGER PRIMARY KEY REFERENCES live_events(id), symbol TEXT NOT NULL,
 bar_end TEXT NOT NULL, price INTEGER NOT NULL, volume INTEGER NOT NULL);
CREATE INDEX IF NOT EXISTS live_pending_time ON live_pending(symbol,bar_end,event_id);";

pub struct Store {
    conn: Connection,
    timezone: String,
    policy: &'static str,
    windows: tick_bars::Aggregator,
    stock_windows: Option<tick_bars::Aggregator>,
    expiries: serde_json::Value,
}

fn sql<T>(result: rusqlite::Result<T>) -> Result<T, String> {
    result.map_err(|e| e.to_string())
}

pub(crate) fn time(value: &str) -> Result<NaiveDateTime, String> {
    NaiveDateTime::parse_from_str(&database::timestamp(value, false)?, "%Y-%m-%dT%H:%M:%S%.f")
        .map_err(|e| e.to_string())
}

pub(crate) fn stamp(value: NaiveDateTime) -> String {
    value.format("%Y-%m-%dT%H:%M:%S%.9f").to_string()
}

impl Store {
    pub fn open(root: &Path, path: &Path, timezone: &str, taifex: bool) -> Result<Self, String> {
        if timezone.trim().is_empty()
            || timezone == "unspecified"
            || (taifex && timezone != "Asia/Taipei")
        {
            return Err("請指定行情時區，臺期所模式須用 Asia/Taipei".into());
        }
        canonical_market::initialize(root, path)?;
        let conn = database::open(path)?;
        sql(tick_store::ensure_live_schema(&conn))?;
        sql(conn.execute_batch(SCHEMA))?;
        let (policy, windows) = if taifex {
            ("taifex_regular", tick_bars::Aggregator::with_sessions(60)?)
        } else {
            ("clock_minute", tick_bars::Aggregator::new(60, 0)?)
        };
        Ok(Self {
            conn,
            timezone: timezone.into(),
            policy,
            windows,
            stock_windows: taifex.then(|| tick_bars::Aggregator::with_stock_sessions(60)).transpose()?,
            expiries: if taifex { crate::live_strategy::expiry_overrides(root)? } else { serde_json::Value::Null },
        })
    }

    pub fn ingest(&mut self, raw: &str) -> Result<(), String> {
        let result = self.apply(raw);
        if let Err(reason) = &result {
            sql(self.conn.execute(
                "INSERT INTO live_rejections(body,reason) VALUES(?1,?2)",
                params![raw, reason],
            ))?;
        }
        result
    }

    fn apply(&mut self, raw: &str) -> Result<(), String> {
        let msg: Envelope = serde_json::from_str(raw).map_err(|e| e.to_string())?;
        if msg.protocol_version != 1
            || msg.event_id <= 0
            || [&msg.provider_id, &msg.connection_id, &msg.connection_epoch]
                .iter()
                .any(|s| s.trim().is_empty())
        {
            return Err("行情協定版本或事件識別無效".into());
        }
        let tx = sql(self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate))?;
        let prior: Option<String> = sql(tx.query_row("SELECT body FROM live_events WHERE provider=?1 AND connection=?2 AND epoch=?3 AND event_id=?4",
            params![msg.provider_id,msg.connection_id,msg.connection_epoch,msg.event_id],|r|r.get(0)).optional())?;
        if let Some(prior) = prior {
            return if prior == raw {
                Ok(())
            } else {
                Err("相同事件識別對應不同內容".into())
            };
        }
        sql(tx.execute("INSERT INTO live_events(provider,connection,epoch,event_id,body) VALUES(?1,?2,?3,?4,?5)",
            params![msg.provider_id,msg.connection_id,msg.connection_epoch,msg.event_id,raw]))?;
        let row = tx.last_insert_rowid();
        match &msg.event {
            Event::Trade {
                instrument,
                event_time,
                price,
                quantity,
                foreign_trade_key,
            } => {
                let symbol = instrument.trim().to_ascii_uppercase();
                if symbol.is_empty() {
                    return Err("商品識別不得空白".into());
                }
                let dt = time(event_time)?;
                let stock = matches!(symbol.as_bytes(), [b'C', b'D', b'F', b'A'..=b'L', b'0'..=b'9']);
                let (policy, windows) = match self.stock_windows.as_ref().filter(|_| stock) {
                    Some(windows) => (STOCK_POLICY, windows),
                    None => (self.policy, &self.windows),
                };
                // The exact settlement print still belongs to the expiring contract.
                let window_at = if policy == STOCK_POLICY
                    && dt.time() == chrono::NaiveTime::from_hms_opt(13, 30, 0).unwrap()
                    && crate::live_strategy::monthly_quote_contract("CDF", dt, &self.expiries)
                        .is_some_and(|(_, expected, paused)| paused && expected == symbol)
                {
                    dt - chrono::TimeDelta::nanoseconds(1)
                } else {
                    dt
                };
                let (_, end) = windows.window(window_at)
                    .ok_or(OUT_OF_SESSION)?;
                let bar_end = stamp(
                    DateTime::from_timestamp(end, 0)
                        .ok_or("收棒時間超限")?
                        .naive_utc(),
                );
                let price = i64::try_from(tick_bars::decimal(price)?).map_err(|_| "成交價超限")?;
                let volume =
                    i64::try_from(tick_bars::decimal(quantity)?).map_err(|_| "成交量超限")?;
                if volume <= 0 {
                    return Err("成交量須大於零".into());
                }
                if let Some(key) = foreign_trade_key {
                    if key.is_empty() || key.len() > 128 {
                        return Err("海外成交識別格式錯誤".into());
                    }
                    let args = params![msg.provider_id, msg.connection_id, symbol, key];
                    let prior = sql(tx.query_row(
                        "SELECT price,volume FROM live_foreign_trade_keys WHERE provider=?1 AND connection=?2 AND symbol=?3 AND trade_key=?4",
                        args, |row| Ok((row.get::<_,i64>(0)?,row.get::<_,i64>(1)?)),
                    ).optional())?;
                    if let Some(prior) = prior {
                        return if prior == (price, volume) { Ok(()) } else {
                            Err("同一海外成交識別有不同價量".into())
                        };
                    }
                    sql(tx.execute("INSERT INTO live_foreign_trade_keys VALUES(?1,?2,?3,?4,?5,?6)",
                        params![msg.provider_id, msg.connection_id, symbol, key, price, volume]))?;
                }
                let (tick, minute) = source(&tx, &msg, &symbol, &self.timezone, policy)?;
                let cutoff: Option<String> = sql(tx.query_row(
                    "SELECT cutoff FROM live_watermarks WHERE provider=?1 AND connection=?2 AND timezone=?3 AND session_policy=?4",
                    params![msg.provider_id,msg.connection_id,self.timezone,policy], |r| r.get(0),
                ).optional())?;
                let last: Option<String> = sql(tx.query_row(
                    "SELECT last_datetime FROM datasets WHERE id=?1",
                    [minute],
                    |r| r.get(0),
                ))?;
                if last.is_some_and(|last| last >= bar_end)
                    || cutoff.is_some_and(|cutoff| cutoff >= bar_end)
                {
                    return Err(SEALED_MINUTE.into());
                }
                tick_store::append_live(&tx, tick, dt, price, volume)?;
                sql(tx.execute(
                    "INSERT INTO live_pending VALUES(?1,?2,?3,?4,?5)",
                    params![row, symbol, bar_end, price, volume],
                ))?;
                // A later trade proves we crossed the earlier minute boundary.
                close_bars(
                    &tx,
                    &msg,
                    &self.timezone,
                    policy,
                    &bar_end,
                    false,
                    Some(&symbol),
                )?;
            }
            Event::Watermark { event_time } => {
                let cutoff = stamp(time(event_time)?);
                for policy in [Some(self.policy), self.stock_windows.as_ref().map(|_| STOCK_POLICY)].into_iter().flatten() {
                    sql(tx.execute("INSERT INTO live_watermarks VALUES(?1,?2,?3,?4,?5)
                    ON CONFLICT(provider,connection,timezone,session_policy) DO UPDATE SET cutoff=max(cutoff,excluded.cutoff)",
                        params![msg.provider_id,msg.connection_id,self.timezone,policy,cutoff]))?;
                    close_bars(&tx, &msg, &self.timezone, policy, &cutoff, true, None)?;
                }
            }
            Event::Status { state } => {
                eprintln!("{} [{}] {}", msg.provider_id, msg.connection_id, state)
            }
        }
        sql(tx.commit())
    }
}

fn source(
    tx: &Transaction<'_>,
    msg: &Envelope,
    symbol: &str,
    timezone: &str,
    policy: &str,
) -> Result<(i64, i64), String> {
    let current: Option<(String,String,String,String,i64,i64)> = sql(tx.query_row(
        "SELECT provider,connection,timezone,session_policy,tick_id,minute_id FROM live_sources WHERE symbol=?1",
        [symbol], |r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?))).optional())?;
    if let Some((provider, connection, zone, session, tick, minute)) = current {
        if provider == msg.provider_id
            && connection == msg.connection_id
            && zone == timezone
            && session == policy
        {
            let count: i64 = sql(tx.query_row("SELECT count(*) FROM market_sources WHERE symbol=?1 AND ((kind='tick' AND dataset_id=?2) OR (kind='minute' AND dataset_id=?3))",
                params![symbol,tick,minute], |r| r.get(0)))?;
            if count == 2 {
                return Ok((tick, minute));
            }
        }
        return Err("商品已有不同的行情來源或交易時段設定".into());
    }
    let mut ids = Vec::new();
    for (kind, stored, role) in [("tick", "trade", "event"), ("minute", "minute", "end")] {
        sql(tx.execute("INSERT INTO datasets(symbol,source,kind,timezone,timestamp_role,path) VALUES(?1,'live',?2,?3,?4,?5)",
            params![symbol,stored,timezone,role,format!("live:{}",msg.provider_id)]))?;
        let id = tx.last_insert_rowid();
        sql(tx.execute(
            "INSERT INTO market_sources VALUES(?1,?2,?3)",
            params![symbol, kind, id],
        ))
        .map_err(|e| format!("無法建立即時行情來源，請核對商品既有來源：{e}"))?;
        ids.push(id);
    }
    let (tick, minute) = (ids[0], ids[1]);
    sql(tx.execute(
        "INSERT INTO tick_canonical VALUES(?1,?2)",
        params![symbol, tick],
    ))?;
    sql(tx.execute(
        "INSERT INTO dataset_storage VALUES(?1,'fixed6_le24_v1')",
        [tick],
    ))?;
    sql(tx.execute(
        "INSERT INTO live_sources VALUES(?1,?2,?3,?4,?5,?6,?7)",
        params![
            symbol,
            msg.provider_id,
            msg.connection_id,
            timezone,
            policy,
            tick,
            minute
        ],
    ))?;
    Ok((tick, minute))
}

fn close_bars(
    tx: &Transaction<'_>,
    msg: &Envelope,
    timezone: &str,
    policy: &str,
    cutoff: &str,
    inclusive: bool,
    instrument: Option<&str>,
) -> Result<(), String> {
    let mut query = sql(tx.prepare(
        "SELECT p.symbol,p.bar_end,s.minute_id,p.price,p.volume
        FROM live_pending p JOIN live_sources s ON s.symbol=p.symbol
        WHERE s.provider=?1 AND s.connection=?2 AND s.timezone=?3 AND s.session_policy=?4
        AND (p.bar_end < ?5 OR (?6 AND p.bar_end=?5)) AND (?7 IS NULL OR p.symbol=?7)
        ORDER BY p.symbol,p.bar_end,p.event_id",
    ))?;
    let mut rows = sql(query.query(params![
        msg.provider_id,
        msg.connection_id,
        timezone,
        policy,
        cutoff,
        inclusive,
        instrument
    ]))?;
    let mut bars: Vec<(String, String, i64, [i128; 5])> = Vec::new();
    while let Some(row) = sql(rows.next())? {
        let symbol: String = sql(row.get(0))?;
        let end: String = sql(row.get(1))?;
        let id: i64 = sql(row.get(2))?;
        let px = i128::from(sql(row.get::<_, i64>(3))?);
        let qty = i128::from(sql(row.get::<_, i64>(4))?);
        if let Some((_, _, _, values)) = bars
            .last_mut()
            .filter(|(s, e, _, _)| *s == symbol && *e == end)
        {
            values[1] = values[1].max(px);
            values[2] = values[2].min(px);
            values[3] = px;
            values[4] += qty;
        } else {
            bars.push((symbol, end, id, [px, px, px, px, qty]));
        }
    }
    drop(rows);
    for (symbol, end, id, prices) in bars {
        let [open, high, low, close, volume] = prices.map(tick_bars::text);
        let payload = serde_json::to_string(&database::Payload::Bar {
            open,
            high,
            low,
            close,
            volume,
        })
        .map_err(|e| e.to_string())?;
        let (ordinal,prior): (i64,Option<String>) = sql(tx.query_row(
            "SELECT COALESCE((SELECT max(ordinal) FROM records WHERE dataset_id=?1),0)+1,sha256 FROM datasets WHERE id=?1",
            [id],|r|Ok((r.get(0)?,r.get(1)?))))?;
        let mut hash = Sha256::new();
        hash.update(prior.unwrap_or_default());
        hash.update(&end);
        hash.update(&payload);
        sql(tx.execute(
            "INSERT INTO market_bar_keys VALUES(?1,?2,?3)",
            params![id, end, ordinal],
        ))?;
        sql(tx.execute(
            "INSERT INTO records VALUES(?1,?2,?3,?3,?4)",
            params![id, ordinal, end, payload],
        ))?;
        sql(tx.execute("UPDATE datasets SET row_count=row_count+1,first_datetime=COALESCE(first_datetime,?2),last_datetime=?2,sha256=?3 WHERE id=?1",
            params![id,end,format!("{:x}",hash.finalize())]))?;
        sql(tx.execute(
            "DELETE FROM live_pending WHERE symbol=?1 AND bar_end=?2",
            params![symbol, end],
        ))?;
    }
    Ok(())
}
