//! 依回測保存的商品交易日曆彙整績效。
use crate::quote_manager::{Instrument, TradingCalendar};
use chrono::{Datelike, NaiveDate};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DateBasis {
    #[default]
    Auto,
    SourceCalendar,
}

#[derive(Default, Serialize)]
struct Period {
    trades: usize,
    wins: usize,
    gross_pnl: f64,
    fees: f64,
    net_pnl: f64,
}

fn date(value: &Value) -> Result<NaiveDate, String> {
    value
        .as_str()
        .and_then(|s| s.get(..10))
        .and_then(|s| NaiveDate::parse_from_str(s, "%Y-%m-%d").ok())
        .ok_or_else(|| "報表日期格式錯誤".into())
}

fn number(value: &Value) -> Result<f64, String> {
    value
        .as_f64()
        .filter(|n| n.is_finite())
        .ok_or_else(|| "報表數值缺漏".into())
}

pub fn performance(r: &Value, profile: &Value, basis: DateBasis) -> Result<Value, String> {
    let trades = r["trades"].as_array().ok_or("報表缺少逐筆交易")?;
    let equity = r["equity"].as_array().ok_or("報表缺少權益")?;
    let last = equity.last().ok_or("報表權益為空")?;
    let warmup = r["config"]["warmup_bars"].as_u64().unwrap_or(0) as usize;
    let active = equity
        .get(warmup..)
        .filter(|rows| !rows.is_empty())
        .ok_or("報表未包含暖機後權益")?;
    let start = date(&active[0]["datetime"])?;
    let end = date(&last["datetime"])?;
    let mut basis_note = "依暖機後有行情的來源曆日統計";
    let is_daily = r["provenance"]["kind"] == "daily" || !r["config"]["interval_days"].is_null();
    let mut basis_key = if is_daily {
        "bar_date"
    } else {
        "source_calendar"
    };
    let calendar = if is_daily {
        basis_note = "依暖機後日 K 資料的日期標記統計";
        None
    } else if matches!(basis, DateBasis::SourceCalendar) {
        None
    } else {
        basis_note = "缺少適用的已確認商品時段或來源時區，依暖機後有行情的來源曆日統計";
        serde_json::from_value::<Instrument>(profile.clone())
            .ok()
            .filter(|i| {
                i.confirmed
                    && i.symbol
                        .eq_ignore_ascii_case(r["provenance"]["symbol"].as_str().unwrap_or(""))
                    && r["provenance"]["timezone"] == i.timezone
                    && i.timezone != "unspecified"
            })
            .and_then(|i| TradingCalendar::new(&i, start, end).ok())
    };
    let trading_dates = calendar.as_ref().and_then(|calendar| {
        let day = |v: &Value, close| calendar.trading_date(v.as_str()?, close);
        let bars = active
            .iter()
            .map(|e| day(&e["datetime"], true))
            .collect::<Option<Vec<_>>>()?;
        let exits = trades
            .iter()
            .map(|t| {
                let close_label = r["tick_execution"].is_null()
                    || t["exit_bar"]
                        .as_u64()
                        .and_then(|bar| equity.get(bar as usize))
                        .is_some_and(|bar| bar["datetime"] == t["exit_datetime"]);
                day(&t["exit_datetime"], close_label)
            })
            .collect::<Option<Vec<_>>>()?;
        // 單日回撤以時間順序計算，每個交易日須為連續區段。
        bars.windows(2)
            .all(|b| b[0] <= b[1])
            .then_some((bars, exits))
    });
    if trading_dates.is_some() {
        basis_key = "trading_date";
        basis_note = "依回測時保存的商品時段、交易星期與手動休市日歸屬；分母為暖機後有行情的交易日";
    } else if calendar.is_some() {
        basis_note = "部分行情或成交無法一致歸屬商品交易日，全部統計依暖機後有行情的來源曆日";
    }
    let (bar_dates, exit_dates) = match trading_dates {
        Some(dates) => dates,
        None => (
            active
                .iter()
                .map(|e| date(&e["datetime"]))
                .collect::<Result<Vec<_>, _>>()?,
            trades
                .iter()
                .map(|t| date(&t["exit_datetime"]))
                .collect::<Result<Vec<_>, _>>()?,
        ),
    };
    let start = exit_dates
        .iter()
        .copied()
        .min()
        .map_or(bar_dates[0], |d| d.min(bar_dates[0]));
    let end = exit_dates
        .iter()
        .copied()
        .max()
        .map_or(*bar_dates.last().unwrap(), |d| {
            d.max(*bar_dates.last().unwrap())
        });
    let mut months = BTreeMap::<String, Period>::new();
    let mut years = BTreeMap::<String, Period>::new();
    let mut month = NaiveDate::from_ymd_opt(start.year(), start.month(), 1).ok_or("月份無效")?;
    while month <= end {
        months.entry(month.format("%Y-%m").to_string()).or_default();
        years.entry(month.format("%Y").to_string()).or_default();
        month = month
            .checked_add_months(chrono::Months::new(1))
            .ok_or("月份超出範圍")?;
    }
    let mut total = Period::default();
    let (mut gain, mut loss, mut losses) = (0.0, 0.0, 0);
    for (t, d) in trades.iter().zip(exit_dates) {
        let (gross, fees, net) = (
            number(&t["gross"])?,
            number(&t["costs"])?,
            number(&t["net"])?,
        );
        for target in [
            &mut total,
            months.entry(d.format("%Y-%m").to_string()).or_default(),
            years.entry(d.format("%Y").to_string()).or_default(),
        ] {
            target.trades += 1;
            target.wins += usize::from(net > 0.0);
            target.gross_pnl += gross;
            target.fees += fees;
            target.net_pnl += net;
        }
        if net > 0.0 {
            gain += net;
        } else if net < 0.0 {
            loss -= net;
            losses += 1;
        }
    }
    let mut streak = Vec::new();
    let mut longest = Vec::new();
    for (key, p) in &months {
        if p.net_pnl < 0.0 {
            streak.push(key);
        } else {
            streak.clear();
        }
        if streak.len() > longest.len() {
            longest = streak.clone();
        }
    }
    let mut days = BTreeSet::new();
    let mut previous = if warmup == 0 {
        number(&r["config"]["initial_capital"])?
    } else {
        number(&equity[warmup - 1]["equity"])?
    };
    let (mut active_day, mut peak, mut upper, mut lower, mut worst_date) =
        (None, previous, 0.0_f64, 0.0_f64, None);
    let mut intrabar = true;
    for (e, d) in active.iter().zip(bar_dates) {
        let close = number(&e["equity"])?;
        days.insert(d);
        if active_day != Some(d) {
            active_day = Some(d);
            peak = previous;
        }
        let bounds = e["equity_low"].as_f64().zip(e["equity_high"].as_f64());
        let (low, high) = bounds.unwrap_or((close, close));
        intrabar &= bounds.is_some();
        let dd = (peak - low).max(high - low);
        if worst_date.is_none() || dd > upper {
            upper = dd;
            worst_date = Some(d.to_string());
        }
        lower = lower.max(peak - low).max(
            e["equity_low_after_high"]
                .as_f64()
                .map_or(0.0, |x| high - x),
        );
        peak = peak.max(high);
        previous = close;
    }
    let summary = &r["summary"];
    Ok(json!({
        "gross_pnl":total.gross_pnl,"commission":number(&summary["commissions"])?,
        "closed_trade_costs":total.fees,"net_pnl":total.net_pnl,"net_including_open":summary["net_total"],
        "trades":total.trades,"trading_days":days.len(),
        "average_daily_trades":(!days.is_empty()).then(||total.trades as f64/days.len() as f64),
        "date_basis":basis_key,"trading_day_basis":basis_note,
        "win_rate_pct":summary["win_rate"],"profit_factor_mc":summary["profit_factor"],
        "payoff_ratio":(total.wins>0&&losses>0).then(||(gain/total.wins as f64)/(loss/losses as f64)),
        "maximum_daily_drawdown":if intrabar && (upper-lower).abs()<0.000001 {Some(upper)} else {None},
        "maximum_daily_drawdown_upper_bound":if intrabar {Some(upper)} else {None},
        "maximum_daily_drawdown_lower_bound":lower,"maximum_daily_drawdown_date":worst_date,
        "maximum_cumulative_drawdown":number(&summary["max_drawdown"])?.abs(),
        "longest_losing_months":longest.len(),"losing_streak_start":longest.first(),"losing_streak_end":longest.last(),
        "yearly":years,"monthly":months
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn night_session_uses_one_trading_date_for_trades_drawdown_and_month() {
        let mut instrument = crate::quote_manager::defaults("TXF1");
        instrument.confirmed = true;
        // 週五夜盤跨過週末及週一休市，歸屬週二。
        instrument.closed_dates = vec!["2026-02-02".into()];
        let profile = json!(instrument);
        let point = |datetime, value| {
            json!({"datetime":datetime,"equity":value,
            "equity_high":value,"equity_low":value,"equity_low_after_high":value})
        };
        let mut report = json!({"provenance":{"symbol":"TXF1","kind":"minute","timezone":"Asia/Taipei"},
            "config":{"initial_capital":100,"warmup_bars":1},
            "summary":{"commissions":1,"net_total":-10,"win_rate":0,"profit_factor":0,"max_drawdown":-30},
            "trades":[{"exit_datetime":"2026-01-30T21:01:00","gross":-9,"costs":1,"net":-10}],
            "equity":[point("2026-01-30T09:01:00",100),point("2026-01-30T21:00:00",120),
                point("2026-01-30T21:01:00",110),point("2026-01-31T05:00:00",90),point("2026-02-03T09:01:00",90)]});
        let trading = performance(&report, &profile, DateBasis::Auto).unwrap();
        assert_eq!(trading["date_basis"], "trading_date");
        assert_eq!(trading["trading_days"], 1);
        assert_eq!(trading["average_daily_trades"], 1.0);
        assert_eq!(trading["maximum_daily_drawdown"], 30.0);
        assert_eq!(trading["maximum_daily_drawdown_date"], "2026-02-03");
        assert_eq!(trading["monthly"]["2026-02"]["net_pnl"], -10.0);
        assert!(trading["monthly"]["2026-01"].is_null());
        let calendar = performance(&report, &profile, DateBasis::SourceCalendar).unwrap();
        assert_eq!(calendar["date_basis"], "source_calendar");
        assert_eq!(calendar["trading_days"], 3);
        assert_eq!(calendar["maximum_daily_drawdown"], 20.0);
        assert_eq!(calendar["maximum_daily_drawdown_date"], "2026-01-31");
        assert_eq!(calendar["monthly"]["2026-01"]["net_pnl"], -10.0);
        report["provenance"]["timezone"] = json!("unspecified");
        let fallback = performance(&report, &profile, DateBasis::Auto).unwrap();
        assert_eq!(fallback["date_basis"], "source_calendar");
        assert_eq!(fallback["monthly"], calendar["monthly"]);
    }
}
