import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import {mcProfitFactor,mcRunLengths,mcDailyDisplayRows,mcClosedDrawdownPercent,mcRunPercents,mcTradeBarAverages} from './mc-report-statistics.mjs';

test('MC原生獲利因子按各母體淨利帶符號，不能按多空方向決定',()=>{
 const evidence=JSON.parse(fs.readFileSync(new URL('../data/mc-profit-factor-reference-vectors.json',import.meta.url),'utf8'));
 assert.ok(evidence.vectors.length>=60);
 for(const row of evidence.vectors)assert.ok(Math.abs(mcProfitFactor(row.profit,row.loss)-row.expected)<1e-10,JSON.stringify(row));
 assert.equal(mcProfitFactor(100,0),'n/a');
});
test('連續損益統計只列實際出現的長度',()=>{
 const run=(sign,n)=>({sign,items:Array(n).fill(0)});
 assert.deepEqual(mcRunLengths([run(-1,1),run(-1,3),run(-1,3),run(1,2),run(-1,14)],-1),[1,3,14]);
});
test('MC 交易 K 棒數納入進出場棒，交易間三欄依原生分類計算',()=>{
 const trades=[{entry_bar:3,exit_bar:3,net:10},{entry_bar:4,exit_bar:6,net:20},
  {entry_bar:6,exit_bar:7,net:-10},{entry_bar:8,exit_bar:11,net:0}];
 assert.deepEqual(mcTradeBarAverages(trades,20),[2.5,2,2,4,8,18]);
 assert.deepEqual(mcTradeBarAverages(trades.slice(0,1),20),[1,1,'n/a','n/a',19,'n/a']);
 assert.equal(mcTradeBarAverages([{entry_bar:0,exit_bar:0,net:1e-9}],20)[3],1);
 assert.equal(mcTradeBarAverages([{entry_bar:0,exit_bar:0,net:1e-8}],20)[3],'n/a');
 assert.deepEqual(mcTradeBarAverages([],20),Array(6).fill('n/a'));
});
test('每日表包含持倉跨越的無行情日，略過空手日',()=>{
 const rows=[{date:'2026-01-01',tradeCount:0,hasBars:true},{date:'2026-01-02',tradeCount:1,hasBars:true},{date:'2026-01-03',tradeCount:1,hasBars:false}];
 assert.deepEqual(mcDailyDisplayRows(rows).map(r=>r.date),['2026-01-02','2026-01-03']);
});
test('回撤百分比與連續獲利百分比使用各自發生時的資本',()=>{
 const trades=[{net:-66},{net:74},{net:74}];
 assert.equal(mcClosedDrawdownPercent(trades,100),-66);
 const pct=mcRunPercents(trades,100);assert.ok(Math.abs(pct.winning-435.2941176470588)<1e-10);assert.equal(pct.losing,-66);
});
