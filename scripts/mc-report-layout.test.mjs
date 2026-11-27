import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import {mcReportChartPlan} from './mc-report-layout.mjs';
const profile=name=>JSON.parse(fs.readFileSync(new URL(`../data/${name}`,import.meta.url),'utf8'));

test('BE5 原MC九張圖不可落入舊版八張圖的配置',()=>{
 const p=mcReportChartPlan(profile('mc-report-layout-be5.json'));
 assert.equal(p.analysisCount,3);assert.equal(p.tradeNames.length,6);assert.deepEqual(p.periodNames,[]);
 assert.equal(p.analysisCount+p.tradeNames.length+p.periodNames.length,9);
 assert.equal(p.tradeNames.at(-1),'mae-scatter');
});
test('NH14完整二十八張圖維持原有順序',()=>{
 const p=mcReportChartPlan(profile('mc-report-layout-nh14.json'));
 assert.equal(p.analysisCount+p.tradeNames.length+p.periodNames.length,28);
 assert.equal(p.tradeNames.at(-1),'mfe-scatter-pct');assert.equal(p.periodNames.at(-1),'annual-cumulative');
});
test('既有預設八張圖不新增策略分析圖片',()=>{
 const p=mcReportChartPlan(profile('mc-report-layout.json'));
 assert.equal(p.analysisCount,0);assert.equal(p.tradeNames.length,2);assert.equal(p.periodNames.length,6);
});
test('不支援或非整數圖表數量明確拒絕',()=>{
 for(const count of [-1,1.5,11])assert.throws(()=>mcReportChartPlan({analysis_chart_count:count}));
});
