//! Reproducible stress tests against a completed OHLC backtest snapshot.
use crate::{
    backtest::{self, Candle, Config, IntrabarPolicy, Provenance, ResultReport},
    powerlanguage::Program,
    strategy_library, workbench,
};
use chrono::{NaiveDate, NaiveDateTime};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{fs, path::Path};

#[derive(Deserialize)]
#[serde(remote = "Provenance")]
struct SavedProvenance {
    outside_session_rows: usize,
    dataset_id: i64,
    symbol: String,
    source: String,
    kind: String,
    timezone: String,
    source_timestamp_role: String,
    source_sha256: String,
    bars_sha256: String,
    source_rows: usize,
    partial_bars: usize,
}

#[derive(Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Options {
    seed: u32,
    repetitions: usize,
    noise_ticks: i32,
    block_bars: usize,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            seed: 20261122,
            repetitions: 20,
            noise_ticks: 1,
            block_bars: 20,
        }
    }
}

struct Random(u64);
impl Random {
    fn index(&mut self, limit: usize) -> usize {
        // SplitMix64. All choices are determined by the saved seed.
        self.0 = self.0.wrapping_add(0x9e3779b97f4a7c15);
        let mut value = self.0;
        value = (value ^ (value >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
        value = (value ^ (value >> 27)).wrapping_mul(0x94d049bb133111eb);
        ((value ^ (value >> 31)) % limit as u64) as usize
    }
}

fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn bars_digest(bars: &[Candle]) -> Result<String, String> {
    Ok(digest(
        &serde_json::to_vec(bars).map_err(|e| e.to_string())?,
    ))
}

fn perturb(bars: &[Candle], tick: f64, bound: i32, rng: &mut Random) -> Vec<Candle> {
    bars.iter()
        .cloned()
        .map(|mut bar| {
            let mut shift = |price: f64| {
                let offset = rng.index((2 * bound + 1) as usize) as i32 - bound;
                ((price / tick).round() + f64::from(offset)) * tick
            };
            bar.open = shift(bar.open);
            bar.close = shift(bar.close);
            bar.high = shift(bar.high).max(bar.open).max(bar.close);
            bar.low = shift(bar.low).min(bar.open).min(bar.close);
            bar
        })
        .collect()
}

fn consecutive_lengths(bars: &[Candle], cfg: &Config, daily: bool) -> Result<Vec<usize>, String> {
    if daily {
        // Daily bars are adjacent trading observations even across holidays.
        for bar in bars {
            NaiveDate::parse_from_str(&bar.datetime, "%Y-%m-%d").map_err(|e| e.to_string())?;
        }
        return Ok(vec![bars.len(); bars.len()]);
    }
    let times: Vec<_> = bars
        .iter()
        .map(|bar| {
            NaiveDateTime::parse_from_str(&bar.datetime, "%Y-%m-%dT%H:%M:%S%.f")
                .map_err(|e| e.to_string())
        })
        .collect::<Result<_, _>>()?;
    let seconds = cfg
        .interval_seconds
        .map(i64::from)
        .unwrap_or(i64::from(cfg.interval_minutes) * 60);
    let mut lengths = vec![1; bars.len()];
    for i in (1..times.len()).rev() {
        if times[i]
            .signed_duration_since(times[i - 1])
            .num_nanoseconds()
            == Some(seconds * 1_000_000_000)
        {
            lengths[i - 1] = lengths[i] + 1;
        }
    }
    Ok(lengths)
}

fn bootstrap(
    bars: &[Candle],
    tick: f64,
    block: usize,
    lengths: &[usize],
    starts: &[usize],
    daily: bool,
    rng: &mut Random,
) -> Vec<Candle> {
    let grid = |price: f64| (price / tick).round();
    let mut path: Vec<Candle> = Vec::with_capacity(bars.len());
    while path.len() < bars.len() {
        let destination = path.len();
        let size = block
            .min(lengths[destination])
            .min(bars.len() - destination);
        let start = starts[rng.index(starts.len())];
        for i in start..start + size {
            let j = path.len();
            let sample = &bars[i];
            let mut bar = bars[j].clone();
            let sample_open = grid(sample.open);
            let open = if j == 0 {
                grid(bar.open)
            } else {
                let previous_close = grid(path[j - 1].close);
                let gap = if i == start {
                    if !daily && lengths[j - 1] == 1 {
                        grid(bar.open) - grid(bars[j - 1].close)
                    } else {
                        0.0
                    }
                } else {
                    sample_open - grid(bars[i - 1].close)
                };
                previous_close + gap
            };
            bar.open = open * tick;
            bar.high = (open + (grid(sample.high) - sample_open)) * tick;
            bar.low = (open + (grid(sample.low) - sample_open)) * tick;
            bar.close = (open + (grid(sample.close) - sample_open)) * tick;
            bar.volume = sample.volume;
            path.push(bar);
        }
    }
    path
}

fn outcome(report: &ResultReport) -> Value {
    let violations: Vec<_> = report
        .trades
        .iter()
        .filter(|t| t.exit_bar <= t.entry_bar)
        .collect();
    json!({
        "summary":report.summary, "final_position":report.final_position,
        "pending_at_end":report.pending_at_end, "ambiguous_bars":report.ambiguous_bars,
        "same_bar_violations":violations.len(), "passes_bar_separation":violations.is_empty(),
        "violation_examples":violations.into_iter().take(10).collect::<Vec<_>>(),
        "bars_sha256":report.provenance.bars_sha256,
    })
}

fn distribution(rows: &[Value], baseline_net: f64, baseline_drawdown: f64) -> Value {
    let values = |field| {
        let mut values = rows
            .iter()
            .filter_map(|r| r["result"]["summary"][field].as_f64())
            .collect::<Vec<_>>();
        values.sort_by(f64::total_cmp);
        values
    };
    let net = values("net_closed");
    let drawdown = values("max_drawdown");
    let quantile = |values: &[f64], p: f64| {
        (!values.is_empty()).then(|| {
            let rank = (values.len() - 1) as f64 * p;
            let low = values[rank.floor() as usize];
            let high = values[rank.ceil() as usize];
            low + (high - low) * rank.fract()
        })
    };
    json!({
        "runs":rows.len(), "completed":net.len(), "failed":rows.len()-net.len(),
        "positive_net":net.iter().filter(|v| **v > 0.0).count(),
        "net_below_baseline":net.iter().filter(|v| **v < baseline_net).count(),
        "drawdown_worse_than_baseline":drawdown.iter().filter(|v| **v < baseline_drawdown).count(),
        "zero_trade_runs":rows.iter().filter(|r| r["result"]["summary"]["trades"] == 0).count(),
        "open_position_runs":rows.iter().filter(|r| r["result"]["final_position"].as_i64().is_some_and(|v| v != 0)).count(),
        "same_bar_violation_runs":rows.iter().filter(|r| r["result"]["same_bar_violations"].as_u64().unwrap_or(0)>0).count(),
        "distinct_completed_paths":rows.iter().filter_map(|r| r["result"]["bars_sha256"].as_str())
            .collect::<std::collections::BTreeSet<_>>().len(),
        "net_closed":{"p05":quantile(&net,0.05),"median":quantile(&net,0.5),"p95":quantile(&net,0.95)},
        "max_drawdown":{"p05":quantile(&drawdown,0.05),"median":quantile(&drawdown,0.5),"p95":quantile(&drawdown,0.95)}
    })
}

pub fn run(
    root: &Path,
    run_id: &str,
    options: Options,
    progress: impl FnMut(String),
) -> Result<Value, String> {
    run_controlled(root, run_id, options, progress, None)
}

pub fn run_controlled(
    root: &Path,
    run_id: &str,
    options: Options,
    mut progress: impl FnMut(String),
    stop: Option<&std::sync::atomic::AtomicBool>,
) -> Result<Value, String> {
    let check_stop = || {
        if stop.is_some_and(|flag| flag.load(std::sync::atomic::Ordering::Acquire)) {
            Err("工作已取消".to_owned())
        } else { Ok(()) }
    };
    check_stop()?;
    if !run_id.strip_prefix("rust-").is_some_and(|s| {
        !s.is_empty() && s.len() <= 100 && s.bytes().all(|c| c.is_ascii_digit() || c == b'-')
    }) {
        return Err("回測編號無效".into());
    }
    if !(1..=100).contains(&options.repetitions)
        || !(1..=10).contains(&options.noise_ticks)
        || !(2..=500).contains(&options.block_bars)
    {
        return Err("情境次數須為 1 至 100、雜訊為 1 至 10 跳、區塊長度為 2 至 500 棒".into());
    }
    let dir = root.join("outputs/pl-workbench").join(run_id);
    let receipt = workbench::read_json(&dir.join("result.json"))?;
    if receipt["status"] != "completed" {
        return Err("請選擇已完成的回測".into());
    }
    let mut saved = workbench::read_json(&dir.join("engine/report.json"))?;
    let cfg: Config = serde_json::from_value(saved["config"].clone()).map_err(|e| e.to_string())?;
    if cfg.tick_dataset_id.is_some() {
        return Err("此模組使用 OHLC 棒內撮合，逐筆撮合基準須先建立獨立 OHLC 基準".into());
    }
    let provenance =
        SavedProvenance::deserialize(saved["provenance"].clone()).map_err(|e| e.to_string())?;
    // The saved report can contain large equity and order histories.
    saved.as_object_mut().unwrap().retain(|key, _| {
        matches!(
            key.as_str(),
            "strategy_sha256" | "configuration_sha256" | "engine_sha256" | "summary"
        )
    });
    let source = fs::read_to_string(dir.join("input.pl")).map_err(|e| e.to_string())?;
    let bars: Vec<Candle> = csv::Reader::from_path(dir.join("engine/bars.csv"))
        .map_err(|e| e.to_string())?
        .deserialize()
        .collect::<Result<_, _>>()
        .map_err(|e| e.to_string())?;
    if saved["strategy_sha256"] != digest(source.as_bytes())
        || saved["configuration_sha256"]
            != digest(&serde_json::to_vec(&cfg).map_err(|e| e.to_string())?)
        || provenance.bars_sha256 != bars_digest(&bars)?
    {
        return Err("策略、設定或行情快照與原回測雜湊不同".into());
    }
    if bars.len().saturating_mul(2 * options.repetitions + 5) > 10_000_000 {
        return Err("情境總棒數超過一千萬，請減少情境次數或縮短基準期間".into());
    }
    cfg.validate()?;
    if bars.is_empty() {
        return Err("基準行情快照沒有 K 棒".into());
    }
    if bars.iter().any(|bar| {
        [bar.open, bar.high, bar.low, bar.close]
            .iter()
            .any(|price| {
                let units = price / cfg.tick_size;
                !units.is_finite() || (units - units.round()).abs() > 1e-6
            })
    }) {
        return Err("基準行情價格未對齊最小跳動，請先核對商品規格與價格".into());
    }
    let daily = provenance.kind == "daily";
    let lengths = consecutive_lengths(&bars, &cfg, daily)?;
    let starts: Vec<_> = lengths
        .iter()
        .enumerate()
        .filter_map(|(i, n)| {
            (*n >= options.block_bars && bars.len() - i >= options.block_bars).then_some(i)
        })
        .collect();
    if starts.len() < 2 {
        return Err("可抽樣的連續區塊不足，請縮短區塊長度或擴大基準期間".into());
    }
    let program = Program::with_functions(&source, &cfg.inputs, &cfg.functions)?;
    let evaluate = |scenario: &[Candle], config: &Config| {
        check_stop()?;
        let mut proof = provenance.clone();
        proof.bars_sha256 = bars_digest(scenario)?;
        backtest::run(scenario, &program, config, proof, &source)
    };
    let baseline = evaluate(&bars, &cfg)?;
    if saved["engine_sha256"] != baseline.engine_sha256 {
        return Err("目前引擎與基準版本不同，請重新執行基準回測".into());
    }
    let reproduced = json!(baseline.summary);
    let matches_summary = reproduced
        .as_object()
        .unwrap()
        .iter()
        .all(|(field, value)| {
            let original = &saved["summary"][field];
            if let (Some(a), Some(b)) = (original.as_f64(), value.as_f64()) {
                (a - b).abs() <= 4.0 * f64::EPSILON * a.abs().max(b.abs()).max(1.0)
            } else {
                original == value
            }
        });
    if !matches_summary {
        return Err("目前引擎無法重現基準績效，請核對原始回測".into());
    }
    let mut groups = serde_json::Map::new();
    let mut scenarios = Vec::new();
    let mut run_group =
        |name: &str, count: usize, scenario: &mut dyn FnMut(usize) -> (Vec<Candle>, Config)| -> Result<(), String> {
            let mut rows = Vec::with_capacity(count);
            for index in 0..count {
                check_stop()?;
                progress(format!("強健性檢測 {name} {}/{}", index + 1, count));
                let (candles, config) = scenario(index);
                let result = evaluate(&candles, &config);
                let mut row = json!({"method":name,"trial":index+1,
                "commission_per_side":config.commission_per_side,"slippage_ticks":config.slippage_ticks,
                "intrabar_policy":config.intrabar_policy});
                match result {
                    Ok(value) => row["result"] = outcome(&value),
                    Err(error) => row["error"] = json!(error),
                }
                rows.push(row);
            }
            groups.insert(
                name.into(),
                distribution(
                    &rows,
                    baseline.summary.net_closed,
                    baseline.summary.max_drawdown,
                ),
            );
            scenarios.extend(rows);
            Ok(())
        };
    let mut noise_rng = Random(u64::from(options.seed));
    run_group("ohlc_noise", options.repetitions, &mut |_| {
        (
            perturb(&bars, cfg.tick_size, options.noise_ticks, &mut noise_rng),
            cfg.clone(),
        )
    })?;
    let mut block_rng = Random(u64::from(options.seed) ^ 0x4a1fdec52bfe6199);
    run_group("moving_block_bootstrap", options.repetitions, &mut |_| {
        (
            bootstrap(
                &bars,
                cfg.tick_size,
                options.block_bars,
                &lengths,
                &starts,
                daily,
                &mut block_rng,
            ),
            cfg.clone(),
        )
    })?;
    run_group("execution_stress", 3, &mut |index| {
        let mut config = cfg.clone();
        if index < 2 {
            config.slippage_ticks += index as u32 + 1;
            config.commission_per_side *= [1.25, 1.5][index];
        } else {
            config.intrabar_policy = match cfg.intrabar_policy {
                IntrabarPolicy::NearestExtreme => IntrabarPolicy::Adverse,
                IntrabarPolicy::Adverse => IntrabarPolicy::NearestExtreme,
            };
        }
        (bars.clone(), config)
    })?;
    check_stop()?;
    let output = root
        .join("outputs/strategy-robustness")
        .join(strategy_library::fresh_id());
    fs::create_dir_all(output.parent().unwrap()).map_err(|e| e.to_string())?;
    fs::create_dir(&output).map_err(|e| e.to_string())?;
    let path = output.join("report.json");
    let summary = json!({
        "schema":"strategy-robustness-v1", "baseline_run_id":run_id, "options":options,
        "strategy_sha256":baseline.strategy_sha256,"engine_sha256":baseline.engine_sha256,
        "configuration_sha256":baseline.configuration_sha256, "baseline":outcome(&baseline),
        "bar_count":bars.len(),"eligible_block_starts":starts.len(),
        "groups":groups, "report_path":path,
        "synthetic_data":true,
        "validation":"人工壓力情境，需另做真實留出期間與上線驗收"
    });
    let mut full = summary.clone();
    full["baseline_warnings"] = json!(baseline.warnings);
    full["scenarios"] = json!(scenarios);
    let temporary = output.join("report.tmp");
    workbench::write_json(&temporary, &full)?;
    fs::rename(temporary, path).map_err(|e| e.to_string())?;
    Ok(summary)
}
