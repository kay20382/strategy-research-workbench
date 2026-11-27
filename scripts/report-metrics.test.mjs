import test from 'node:test';
import assert from 'node:assert/strict';
import {computeMcReportMetrics,mcDuration,mcMagnitudeRatio} from './report-metrics.mjs';

const settings={interest_rate:2,minimum_acceptable_return:2,risk_aversion:'保守'};
function fixture(){
  return {config:{initial_capital:100,warmup_bars:0},summary:{net_closed:-12,max_drawdown:-22},trades:[{net:-12,cumulative:-12,side:1,mfe:10,mae:-12,entry_bar:0,exit_bar:2,entry_datetime:'2020-01-01T00:00:00',exit_datetime:'2020-03-31T00:00:00'}],equity:[
    {datetime:'2020-01-01T00:00:00',equity:100,equity_low:100,equity_low_after_high:100,bar_drawdown:0},
    {datetime:'2020-01-31T00:00:00',equity:110,equity_low:100,equity_low_after_high:100,bar_drawdown:0},
    {datetime:'2020-03-31T00:00:00',equity:88,equity_low:88,equity_low_after_high:88,bar_drawdown:-22},
  ]};
}
test('calendar gaps retain a zero-return month and use below-target population SD',()=>{
  const x=computeMcReportMetrics(fixture(),settings);
  assert.deepEqual(x.monthly.map(m=>m.return),[.1,0,-.2]);
  assert.deepEqual(x.monthly.map(m=>m.opening_equity),[100,110,110]);
  assert.equal(x.downside_deviation,.1);
  assert.ok(Math.abs(x.cells.B42-((-1/30-.02/12)/.1))<1e-12);
  assert.equal(x.cells.B39,'n/a'); // Native UPR requires at least 36 calendar months.
  assert.equal(x.cells.B61,100);
});
test('missing risk settings use the native reference defaults for all six ratios',()=>{
  const defaults=computeMcReportMetrics(fixture());
  const explicit=computeMcReportMetrics(fixture(),settings);
  assert.deepEqual(defaults.risk_settings,settings);
  assert.deepEqual(defaults.defaulted_risk_settings,Object.keys(settings));
  for(let row=40;row<=45;row++){
    assert.ok(Number.isFinite(defaults.cells['B'+row]));
    assert.equal(defaults.cells['B'+row],explicit.cells['B'+row]);
  }
  assert.deepEqual(explicit.defaulted_risk_settings,[]);
});
test('explicit zero rates and the risk coefficient take precedence over defaults',()=>{
  const m=computeMcReportMetrics(fixture(),{interest_rate:0,minimum_acceptable_return:0,risk_aversion_coefficient:0});
  assert.deepEqual(m.defaulted_risk_settings,[]);
  assert.deepEqual(m.risk_settings,{interest_rate:0,minimum_acceptable_return:0,risk_aversion:0});
  assert.equal(m.cells.B44,0);
  assert.equal(m.cells.B45,0);
  assert.ok(Math.abs(m.cells.B43-(-1/30))<1e-12);
});
test('time begins after warmup and overlapping holdings count once',()=>{
  const r=fixture();r.config.warmup_bars=1;
  r.trades.push({...r.trades[0],entry_datetime:'2020-02-01T00:00:00',net:0,cumulative:-12});
  const x=computeMcReportMetrics(r,settings);
  assert.equal(x.period_start,'2020-01-31T00:00:00');assert.equal(x.period_days,60);
  assert.equal(x.occupied_days,60);assert.equal(x.cells.B61,100);
});
test('missing intrabar observations do not become fabricated drawdown ratios',()=>{
  const r=fixture();for(const e of r.equity){delete e.equity_low;delete e.bar_drawdown;}
  const x=computeMcReportMetrics(r,settings);
  assert.equal(x.has_intrabar,false);assert.equal(x.cells.B44,undefined);assert.equal(x.cells.B65,undefined);
});
test('reviewed native duration and magnitude-ratio conventions are explicit',()=>{
  assert.equal(mcDuration(2421+255/1440),'6 年 7 個月 26 天 4 小時 15 分鐘');
  assert.equal(mcDuration(2408.875),'6 年 7 個月 13 天 21 小時');
  assert.equal(mcMagnitudeRatio(1,4),-.25);assert.equal(mcMagnitudeRatio(4,1),4);assert.equal(mcMagnitudeRatio(0,0),0);
});
test('monthly risk uses native post-peak fill-bar values and retains empty synthetic ranges',()=>{
  const r=fixture();
  r.trades=[{...r.trades[0],entry_bar:0,exit_bar:1,exit_datetime:'2020-01-31T00:00:00'},
    {...r.trades[0],entry_bar:2,exit_bar:2,entry_datetime:'2020-03-31T00:00:00'}];
  const m=computeMcReportMetrics(r,settings);
  assert.equal(m.monthly[2].risk_ranges[0].before_equity,100); // prior fill bar's post-peak low, not its 110 closing mark
  const gap=computeMcReportMetrics(fixture(),settings).monthly[1];
  assert.deepEqual(gap.risk_ranges,[{start_bar:2,end_bar:1,before_equity:110,minimum_equity:88}]);
  assert.equal(gap.drawdown,-22);assert.equal(gap.return,0);
  assert.deepEqual(m.provisional_cells,[]);
});
