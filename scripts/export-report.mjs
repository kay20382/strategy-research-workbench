// Deterministic report exporter. Consumes engine JSON; does not call an AI service.
import fs from 'node:fs/promises';
import path from 'node:path';
import os from 'node:os';
import { createRequire } from 'node:module';
import { pathToFileURL } from 'node:url';
import {constants} from 'node:fs';
import {createHash} from 'node:crypto';
import {reportOutputPath} from './report-output-path.mjs';
import {addMcReportPictures,applyMcReportLayout} from './mc-report-layout.mjs';
import { computeMcReportMetrics } from './report-metrics.mjs';
import {mcReportValidationNote} from './mc-report-validation-note.mjs';
import {mcProfitFactor,mcRunLengths,mcDailyDisplayRows,mcClosedDrawdownPercent,mcRunPercents,mcTradeBarAverages} from './mc-report-statistics.mjs';
const dependencyRoot=process.env.ARTIFACT_NODE_ROOT || path.join(os.homedir(),'.cache/codex-runtimes/codex-primary-runtime/dependencies/node');
const bundledRequire=createRequire(path.join(dependencyRoot,'bundle-entry.cjs'));
const {Workbook,SpreadsheetFile}=await import(pathToFileURL(bundledRequire.resolve('@oai/artifact-tool')).href);
const [input,outputArgument,auditPath,strategySourcePath,taskSettingsPath,transport,performancePath,style='full']=process.argv.slice(2);
if(transport&&!['html','csv'].includes(transport))throw new Error('報表輸出格式須為 html 或 csv');
if(!['full','simple'].includes(style)||(!transport&&style==='simple'))throw new Error('簡式報告指定 HTML 或 CSV 格式');
const simple=style==='simple';
if(!input)throw new Error('用法：node scripts/export-report.mjs report.json [新報告.xlsx|-] [audit.json|-] [strategy.pl|-] [task.json]；省略輸出或使用 - 時依預設位置與檔名輸出');
const r=JSON.parse(await fs.readFile(input,'utf8'));
const outputPreferences=JSON.parse(await fs.readFile(new URL('../data/report-output-settings.json',import.meta.url),'utf8'));
const destination=reportOutputPath(r,outputArgument,outputPreferences);
const automaticOutput=!outputArgument||outputArgument==='-';
const output=automaticOutput?path.resolve('outputs','report-exports',r.configuration_sha256||'unversioned',path.win32.basename(destination)):destination;
for(const candidate of new Set([destination,output]))try{await fs.access(candidate);throw new Error(`輸出檔已存在，未覆寫：${candidate}`);}catch(e){if(e.code!=='ENOENT')throw e;}
console.log(`已載入回測結果：${r.bar_count} 根 K 棒、${r.trades.length} 筆已平倉交易`);
if(![1,2].includes(r.schema_version))throw new Error('不支援的回測報告版本');
const audit=auditPath&&auditPath!=='-'?JSON.parse(await fs.readFile(auditPath,'utf8')):null;
const taskSettings=taskSettingsPath?JSON.parse(await fs.readFile(taskSettingsPath,'utf8')):null;
const reportSettings=taskSettings?.report_settings||{};
if(transport){
 reportSettings.layout_profile='data/mc-report-layout-nh14.json';
 reportSettings.analysis_bars_path ||= path.join(path.dirname(input),'bars.csv');
}
const reportSha=createHash('sha256').update(await fs.readFile(input)).digest('hex');
let comparison=null;
for(const file of reportSettings.mc_comparison_path?[reportSettings.mc_comparison_path]:[path.join(path.dirname(input),'comparison.json'),path.join(path.dirname(input),'..','comparison.json')]){
 try{const candidate=JSON.parse(await fs.readFile(file,'utf8'));if(candidate.engine_report_sha256===reportSha){comparison=candidate;break;}if(reportSettings.mc_comparison_path)throw new Error('MC 比較紀錄與本次報告雜湊不同');}
 catch(e){if(e.code!=='ENOENT')throw e;}
}
reportSettings.validation_note=mcReportValidationNote(r,reportSha,comparison);
const mcLayout=JSON.parse(await fs.readFile(reportSettings.layout_profile||new URL('../data/mc-report-layout.json',import.meta.url),'utf8'));
const reportMetrics=computeMcReportMetrics(r,reportSettings);
Object.assign(reportSettings,reportMetrics.risk_settings);
console.log('報表指標計算完成');
if(audit&&audit.strategy_sha256!==r.strategy_sha256)throw new Error('策略檢查報告與執行原碼雜湊不符');
let sourceInputOrder=[];if(strategySourcePath&&strategySourcePath!=='-'){const source=await fs.readFile(strategySourcePath,'utf8'),declaration=source.match(/\bInputs?\s*:\s*([\s\S]*?);/i);if(declaration)sourceInputOrder=[...declaration[1].matchAll(/(?:^|,)\s*([A-Za-z_][\w]*)\s*\(/g)].map(m=>m[1]);}
const inputValues=new Map(Object.entries(r.inputs).map(([k,v])=>[k.toLowerCase(),v]));
const inputRows=(sourceInputOrder.length?sourceInputOrder:Object.keys(r.inputs)).filter(name=>inputValues.has(name.toLowerCase())).map(name=>[name,inputValues.get(name.toLowerCase())]);
const settingsOffset=Math.max(0,inputRows.length-8);
const strategyName=r.config.strategy_name||'策略研究';
const wb=Workbook.create();
const sheets=Object.fromEntries(['策略分析','交易明細','交易分析','週期性分析','設定'].map(n=>[n,wb.worksheets.add(n)]));
const put=(s,a,v)=>s.getRange(a).values=[[v]];
const matrix=(s,a,v)=>{if(v.length)s.getRange(a).write(v);};
function base(s,columns,last){s.showGridLines=true;s.getRange(`A1:${columns}${last}`).format.font={name:mcLayout.font,size:mcLayout.font_size};s.getRange(`A1:${columns}${last}`).format.rowHeight=mcLayout.row_height;s.getRange(`A1:${columns}${last}`).format.columnWidth=mcLayout.value_column_width;s.getRange('A:A').format.columnWidth=mcLayout.label_column_width;s.getRange(`A1:A${last}`).format.font={name:mcLayout.font,size:mcLayout.font_size,bold:true};s.getRange('A1').format.font={name:mcLayout.font,size:mcLayout.title_size,bold:true};s.getRange('A1').format.rowHeight=mcLayout.title_height;}
function section(s,address,title){put(s,address,title);s.getRange(address).format.font={name:mcLayout.font,bold:true,size:mcLayout.font_size};}
function header(s,range){s.getRange(range).format.font={bold:true,name:mcLayout.font,size:mcLayout.font_size};}
const closedEnd=Math.max(4,r.trades.length*2+3),end=closedEnd+(r.open_trade?2:0);
const trades=sheets['交易明細'];base(trades,'P',transport?3:Math.max(5,end));put(trades,'A1','交易明細');
const headers=['交易編號','委託單編號','類型','訊號','日期','時間','價格','數量','獲利(¤)','獲利(%)','累積獲利(¤)','累積獲利(%)','最大可能獲利(¤)','最大可能獲利(%)','最大可能虧損(¤)','最大可能虧損(%)'];
matrix(trades,'A3',[headers]);header(trades,'A3:P3');trades.getRange('A:P').format.columnWidth=mcLayout.value_column_width;
const split=s=>[s.slice(0,10),s.includes('T')?s.split('T')[1].slice(0,8):'00:00:00'];
const excelDate=s=>{const [y,m,d]=s.split('-').map(Number);return (Date.UTC(y,m-1,d)-Date.UTC(1899,11,30))/86400000;};
const excelTime=s=>s.split(':').reduce((sum,x,i)=>sum+Number(x)*[3600,60,1][i],0)/86400;
function tradeRows(t,i,open=false){
  const [ed,et]=split(t.entry_datetime), [xd,xt]=split(t.exit_datetime);
  const notional=Math.abs(t.entry_price*t.quantity*r.config.point_value);
  const pct=x=>notional>0?x/notional*100:'n/a';
  return [[t.id,i*2+1,t.side>0?'進入Long':'進入Short',t.entry_label,excelDate(ed),excelTime(et),t.entry_price,t.quantity,t.net,pct(t.net),t.cumulative,t.cumulative/r.config.initial_capital*100,t.mfe,pct(t.mfe),t.mae,pct(t.mae)],
    [null,i*2+2,t.side>0?'離開Long':'離開Short',open?'open':t.exit_label,excelDate(xd),excelTime(xt),t.exit_price,t.quantity,null,null,null,null,null,null,null,null]];
}
if(!transport){
const writeTrade=(t,i,open=false)=>{
  const row=open?closedEnd+1:4+i*2;
  matrix(trades,`A${row}`,tradeRows(t,i,open));
  for(const col of ['A','I','J','K','L','M','N','O','P'])trades.mergeCells(`${col}${row}:${col}${row+1}`);
};
r.trades.forEach((t,i)=>writeTrade(t,i));
if(r.open_trade)writeTrade(r.open_trade,r.trades.length,true);
trades.getRange(`G4:P${Math.max(4,end)}`).setNumberFormat('#,##0.00');
trades.getRange(`E4:E${Math.max(4,end)}`).setNumberFormat('yyyy/mm/dd');trades.getRange(`F4:F${Math.max(4,end)}`).setNumberFormat('hh:mm:ss');
if(!r.trades.length){put(trades,'A4','本次沒有成交交易；不是缺少交易明細。');trades.getRange('A4:P4').format.font={name:'Arial',size:11,color:'#9A5512'};}
}
const net=`'交易明細'!$I$4:$I$${closedEnd}`,type=`'交易明細'!$C$4:$C$${closedEnd}`;
const analysis=sheets['策略分析'];base(analysis,'D',69);put(analysis,'A1','策略績效總結果');matrix(analysis,'B3',[['所有交易','多單','空單']]);header(analysis,'A3:D3');
const metricNames={4:'淨利',5:'毛利',6:'毛損',7:'調整後淨利',8:'調整後毛利',9:'調整後毛損',10:'特定淨利',11:'特定毛利',12:'特定毛損',13:'帳戶所需金額',14:'帳戶報酬',15:'初始資本報酬',16:'最大策略虧損（假設路徑）',17:'最大策略虧損 (%)',18:'最大平倉交易虧損',19:'最大平倉交易虧損 (%)',20:'最大的策略虧損報酬',21:'獲利因子',22:'調整獲利因子',23:'特定獲利因子',24:'最大持有契約數量',25:'滑價支付',26:'佣金支付',27:'未平倉部位損益',28:'年報酬率',29:'月報酬率',30:'買進持有績效',31:'平均月報酬',32:'月報酬的標準差',33:'交易總次數',34:'%勝率'};
for(const [row,label]of Object.entries(metricNames)){put(analysis,`A${row}`,label);matrix(analysis,`B${row}`,[['n/a','n/a','n/a']]);}
function stats(list){const wins=list.filter(t=>t.net>0),losses=list.filter(t=>t.net<0);let equity=0,peak=0,dd=0;for(const t of list){equity+=t.net;peak=Math.max(peak,equity);dd=Math.min(dd,equity-peak);}return{n:list.length,w:wins.length,l:losses.length,z:list.length-wins.length-losses.length,net:list.reduce((s,t)=>s+t.net,0),profit:wins.reduce((s,t)=>s+t.net,0),loss:losses.reduce((s,t)=>s+t.net,0),cost:list.reduce((s,t)=>s+t.costs,0),dd,avg:list.length?equity/list.length:'n/a',avgWin:wins.length?wins.reduce((s,t)=>s+t.net,0)/wins.length:'n/a',avgLoss:losses.length?losses.reduce((s,t)=>s+t.net,0)/losses.length:'n/a'};}
const deviation=g=>{
 if(g.length<2)return null;
 const average=g.reduce((total,t)=>total+t.net,0)/g.length;
 return Math.sqrt(g.reduce((total,t)=>total+(t.net-average)**2,0)/(g.length-1));
};
const globalWins=r.trades.filter(t=>t.net>0),globalLosses=r.trades.filter(t=>t.net<0);
const upper=globalWins.length>1?stats(globalWins).avg+deviation(globalWins):Infinity,lower=globalLosses.length>1?stats(globalLosses).avg-deviation(globalLosses):-Infinity;
const groups=[r.trades,r.trades.filter(t=>t.side===1),r.trades.filter(t=>t.side===-1)];
for(let i=0;i<3;i++){
  const col=['B','C','D'][i],s=stats(groups[i]),entry=i===1?'進入Long':'進入Short',filter=i?`,${type},"${entry}"`:'';
  if(transport){
   for(const [row,value]of [[4,s.net],[5,s.profit],[6,s.loss],[15,s.net/r.config.initial_capital*100],[21,mcProfitFactor(s.profit,s.loss)]])put(analysis,`${col}${row}`,value);
  }else{
  analysis.getRange(`${col}4`).formulas=[[i===0?`=SUM(${net})`:`=SUMIFS(${net},${type},"${entry}")`]];
  analysis.getRange(`${col}5`).formulas=[[`=SUMIFS(${net},${net},">0"${filter})`]];
  analysis.getRange(`${col}6`).formulas=[[`=SUMIFS(${net},${net},"<0"${filter})`]];
  analysis.getRange(`${col}15`).formulas=[[`=${col}4/'設定'!$B$${15+settingsOffset}*100`]];
  analysis.getRange(`${col}21`).formulas=[[`=IF(${col}6<0,SIGN(${col}4)*${col}5/(-${col}6),"n/a")`]];
  }
  put(analysis,`${col}18`,s.dd);
  put(analysis,`${col}19`,mcClosedDrawdownPercent(groups[i],r.config.initial_capital));
  put(analysis,`${col}24`,groups[i].length?Math.max(...groups[i].map(t=>t.quantity)):0);
  // 多空最大策略回撤由已核對的逐價重播補入；整筆MFE/MAE沒有先後順序。
  if(i>0){put(analysis,`${col}16`,'n/a');put(analysis,`${col}20`,'n/a');}
  put(analysis,`${col}25`,r.config.slippage_ticks===0?0:'n/a');put(analysis,`${col}26`,i===0?r.summary.commissions:r.fills.filter(f=>(f.is_entry?f.side:-f.side)===(i===1?1:-1)).reduce((total,f)=>total+f.fee,0));
  put(analysis,`${col}13`,-s.dd);put(analysis,`${col}14`,s.dd<0?s.net/-s.dd*100:'n/a');
  const ap=s.w?s.profit*(1-1/Math.sqrt(s.w)):0,al=s.l?s.loss*(1+1/Math.sqrt(s.l)):0;
  put(analysis,`${col}7`,ap+al);put(analysis,`${col}8`,ap);put(analysis,`${col}9`,al);
  const selected=stats(groups[i].filter(t=>t.net<=upper&&t.net>=lower));
  put(analysis,`${col}10`,selected.net);put(analysis,`${col}11`,selected.profit);put(analysis,`${col}12`,selected.loss);
  put(analysis,`${col}22`,mcProfitFactor(ap,al));put(analysis,`${col}23`,mcProfitFactor(selected.profit,selected.loss));
  put(analysis,`${col}33`,s.n);put(analysis,`${col}34`,s.n?s.w/s.n*100:'n/a');
}
put(analysis,'B20',r.summary.max_drawdown<0?r.summary.net_closed/-r.summary.max_drawdown:'n/a');put(analysis,'B16',r.summary.max_drawdown);put(analysis,'B17',-r.summary.max_drawdown_pct);put(analysis,'B27',r.summary.open_pnl);put(analysis,'C27',r.final_position>0?r.summary.open_pnl:0);put(analysis,'D27',r.final_position<0?r.summary.open_pnl:0);
if(!r.trades.length){put(analysis,'A2','零交易：勝率、平均交易與獲利因子不適用；請查看設定頁的原碼檢查。');analysis.getRange('A2:D2').format.font={name:'Arial',size:10,color:'#9A5512'};}
section(analysis,'A37','績效比率');matrix(analysis,'A39',[['獲利能力比率(Upside Potential Ratio)','n/a'],['夏普比率','n/a'],['年度夏普比率','n/a'],['索丁諾比率','n/a'],['Fouse 比率','n/a'],['Calmar 比率','n/a'],['Sterling 比率','n/a']]);
matrix(analysis,'A46',[['淨利／最大損失 (%)'],['淨利／最大交易績效回撤 (%)'],['淨利／最大策略績效回撤 (%)'],['特定淨利／最大損失 (%)'],['特定淨利／最大交易績效回撤 (%)'],['特定淨利／最大策略績效回撤 (%)'],['調整後淨利／最大損失 (%)'],['調整後淨利／最大交易績效回撤 (%)'],['調整後淨利／最大策略績效回撤 (%)']]);
section(analysis,'A57','時間分析');matrix(analysis,'A59',[['交易期間','n/a'],['市場有倉位的時間','n/a'],['市場有倉位的時間百分比','n/a'],['最長空手期間','n/a'],['最大獲利日期','n/a'],['最大虧損日期','n/a'],['最大策略虧損日期','n/a'],['最大平倉交易虧損日期','n/a']]);
analysis.getRange('B4:D34').setNumberFormat('#,##0.00');
analysis.getRange('B4:D34').format.horizontalAlignment='right';analysis.getRange('B33:D33').setNumberFormat('#,##0');
const detail=sheets['交易分析'];base(detail,'D',78);put(detail,'A1','總交易分析');matrix(detail,'B3',[['所有交易','多單','空單']]);header(detail,'A3:D3');
const labels=['交易總次數','未平倉交易總數量','獲利交易次數','虧損交易次數','%勝率','平均交易(獲利 & 虧損)','平均獲利交易','平均虧損交易','平均獲利／平均虧損','最大的交易獲利','最大的交易虧損'];
for(let j=0;j<labels.length;j++)put(detail,`A${j+4}`,labels[j]);
groups.forEach((g,i)=>{const s=stats(g);const vals=[s.n,i===0?(r.final_position?1:0):(Math.sign(r.final_position)===(i===1?1:-1)?1:0),s.w,s.l,s.n?s.w/s.n*100:'n/a',s.avg,s.avgWin,s.avgLoss,typeof s.avgWin==='number'&&typeof s.avgLoss==='number'&&s.avgLoss<0?s.avgWin/-s.avgLoss:'n/a',g.length?Math.max(...g.map(t=>t.net)):'n/a',g.length?Math.min(...g.map(t=>t.net)):'n/a'];vals.forEach((v,j)=>put(detail,`${['B','C','D'][i]}${j+4}`,v));});
section(detail,'A23','極端交易');matrix(detail,'B25',[['全部','正','負']]);header(detail,'A25:D25');
const outlierGroups=[r.trades,globalWins,globalLosses];
['平均交易金額的1個標準差','平均交易金額 +1 標準差','平均交易金額 - 1 標準差','極端交易次數','極端交易損益比'].forEach((v,i)=>put(detail,`A${26+i}`,v));
outlierGroups.forEach((g,i)=>{const d=deviation(g),avg=stats(g).avg,out=g.filter(t=>t.net>upper||t.net<lower);[d??'n/a',d===null?'n/a':avg+d,d===null?'n/a':avg-d,out.length,stats(out).net].forEach((v,j)=>put(detail,`${['B','C','D'][i]}${26+j}`,v));});
const consecutiveRuns=[];let currentRun=[],currentSign=0;
for(let i=0;i<r.trades.length;i++){const t=r.trades[i],sign=Math.sign(t.net);if(!sign){if(currentRun.length)consecutiveRuns.push({sign:currentSign,items:currentRun});currentRun=[];currentSign=0;continue;}if(sign!==currentSign){if(currentRun.length)consecutiveRuns.push({sign:currentSign,items:currentRun});currentRun=[];currentSign=sign;}currentRun.push({index:i,trade:t});}
if(currentRun.length)consecutiveRuns.push({sign:currentSign,items:currentRun});
function runSummary(sign,length){const runs=consecutiveRuns.filter(x=>x.sign===sign&&x.items.length===length),items=runs.flatMap(x=>x.items),nextTotal=runs.reduce((sum,x)=>sum+(r.trades[x.items.at(-1).index+1]?.net||0),0);return{count:runs.length,average:items.length?items.reduce((sum,x)=>sum+x.trade.net,0)/items.length:'n/a',nextAverage:runs.length?nextTotal/runs.length:'n/a'};}
section(detail,'A62','連續獲利');matrix(detail,'B62',[['序列編號','序列的平均獲利','下筆交易的平均虧損']]);header(detail,'A62:D62');
const winningRunLengths=mcRunLengths(consecutiveRuns,1);
winningRunLengths.forEach((length,i)=>{const v=runSummary(1,length),row=63+i;matrix(detail,`A${row}`,[ [length,v.count,v.average,v.nextAverage] ]);});
const losingHeader=63+winningRunLengths.length;
section(detail,`A${losingHeader}`,'連續虧損');matrix(detail,`B${losingHeader}`,[['序列編號','序列的平均虧損','下筆交易的平均獲利']]);header(detail,`A${losingHeader}:D${losingHeader}`);
const losingRunLengths=mcRunLengths(consecutiveRuns,-1);
losingRunLengths.forEach((length,i)=>{const v=runSummary(-1,length),row=losingHeader+1+i;matrix(detail,`A${row}`,[ [length,v.count,v.average,v.nextAverage] ]);});
const maxWinningRun=Math.max(0,...consecutiveRuns.filter(x=>x.sign>0).map(x=>x.items.length)),maxLosingRun=Math.max(0,...consecutiveRuns.filter(x=>x.sign<0).map(x=>x.items.length));
const maxWinningRunAmount=Math.max(0,...consecutiveRuns.filter(x=>x.sign>0).map(x=>x.items.reduce((sum,y)=>sum+y.trade.net,0)));
const maxLosingRunAmount=Math.min(0,...consecutiveRuns.filter(x=>x.sign<0).map(x=>x.items.reduce((sum,y)=>sum+y.trade.net,0)));
const runPercents=mcRunPercents(r.trades,r.config.initial_capital);put(detail,'B56',runPercents.winning);put(detail,'B57',runPercents.losing);
put(detail,'B51',stats(r.trades).avg);put(detail,'B52',maxWinningRun);put(detail,'B53',maxLosingRun);put(detail,'B54',maxWinningRunAmount);put(detail,'B55',maxLosingRunAmount);
const excursionTrades=[...r.trades,...(r.open_trade?[r.open_trade]:[])],excursionPct=(t,key)=>t.entry_price*t.quantity*r.config.point_value?t[key]/(t.entry_price*t.quantity*r.config.point_value)*100:0,mean=values=>values.length?values.reduce((s,v)=>s+v,0)/values.length:0;
const sampleSd=values=>{
 if(values.length<2)return 0;
 const avg=mean(values);
 return Math.sqrt(values.reduce((sum,value)=>sum+(value-avg)**2,0)/(values.length-1));
};
if(excursionTrades.length){
const maxMfeTrade=excursionTrades.reduce((a,b)=>b.mfe>a.mfe?b:a),minMaeTrade=excursionTrades.reduce((a,b)=>b.mae<a.mae?b:a),maxMfePctTrade=excursionTrades.reduce((a,b)=>excursionPct(b,'mfe')>excursionPct(a,'mfe')?b:a),minMaePctTrade=excursionTrades.reduce((a,b)=>excursionPct(b,'mae')<excursionPct(a,'mae')?b:a),mfeValues=excursionTrades.map(t=>t.mfe),maeValues=excursionTrades.map(t=>t.mae),mfePctValues=excursionTrades.map(t=>excursionPct(t,'mfe')),maePctValues=excursionTrades.map(t=>excursionPct(t,'mae')),avgMfe=mean(mfeValues),avgMae=mean(maeValues),sdMfe=sampleSd(mfeValues),sdMae=sampleSd(maeValues);
put(detail,'B36',maxMfeTrade.mfe);put(detail,'C36',minMaeTrade.mae);put(detail,'B37',excelDate(split(maxMfeTrade.exit_datetime)[0]));put(detail,'C37',excelDate(split(minMaeTrade.exit_datetime)[0]));put(detail,'B38',avgMfe);put(detail,'C38',avgMae);put(detail,'B39',excursionPct(maxMfePctTrade,'mfe'));put(detail,'C39',excursionPct(minMaePctTrade,'mae'));put(detail,'B40',excelDate(split(maxMfePctTrade.entry_datetime)[0]));put(detail,'C40',excelDate(split(minMaePctTrade.entry_datetime)[0]));put(detail,'B41',mean(mfePctValues));put(detail,'C41',mean(maePctValues));put(detail,'B42',sdMfe);put(detail,'C42',sdMae);put(detail,'B43',avgMfe+sdMfe);put(detail,'C43',avgMae+sdMae);put(detail,'B44',avgMfe-sdMfe);put(detail,'C44',avgMae-sdMae);
}
groups.forEach((trades,i)=>{
 const column=['B','C','D'][i];
 mcTradeBarAverages(trades,r.bar_count).forEach((value,index)=>put(detail,`${column}${15+index}`,value));
});
detail.getRange('B4:D30').setNumberFormat('#,##0.00');
detail.getRange('B63:D81').setNumberFormat('#,##0.00');
detail.getRange('B4:D14').format.horizontalAlignment='right';detail.getRange('B4:D7').setNumberFormat('#,##0');detail.getRange('B29:D29').setNumberFormat('#,##0');
// Format the calendar report tables, not 100k+ rows of chart audit samples.
// The bound covers two daily tables, two monthly tables, two annual tables,
// monthly summaries, headings and spacing. Chart source values remain complete.
const formatDays=Math.max(1,Math.ceil((Date.parse(r.last_bar)-Date.parse(r.first_bar))/86400000)+1);
const formatRows=formatDays*2+Math.ceil(formatDays/28)*2+Math.ceil(formatDays/365)*2+200;
const periods=sheets['週期性分析'];base(periods,'G',formatRows);periods.getRange('A:A').format.columnWidth=mcLayout.period_date_width;periods.getRange('F:F').format.columnWidth=mcLayout.period_date_width;
// MultiCharts period analysis is calendar based and marks open positions to the
// final bar of each date. Allocate each trade's daily PnL by its daily mark and
// charge each side's commission on its execution date.
const dayKey=s=>s.slice(0,10),addDays=(key,n)=>{const d=new Date(`${key}T00:00:00Z`);d.setUTCDate(d.getUTCDate()+n);return d.toISOString().slice(0,10);};
const serialDay=key=>{const [y,m,d]=key.split('-').map(Number);return (Date.UTC(y,m-1,d)-Date.UTC(1899,11,30))/86400000;};
const dateLabel=key=>{const [y,m,d]=key.split('-');return `${Number(y)}/${Number(m)}/${Number(d)}`;};
const equityByDay=new Map();for(const e of r.equity)equityByDay.set(dayKey(e.datetime),e);
const closedEntryIds=new Set(r.trades.map(t=>t.entry_order_id));
const unclosedEntry=[...(r.fills||[])].reverse().find(f=>f.is_entry&&!closedEntryIds.has(f.order_id));
const dailyTrades=r.trades.map(t=>({...t,entry_date:dayKey(t.entry_datetime),exit_date:dayKey(t.exit_datetime),closed:true}));
if(r.final_position&&unclosedEntry&&r.equity.length){const last=r.equity.at(-1),mark=unclosedEntry.price+(last.equity-last.cash)/(unclosedEntry.side*r.config.point_value*unclosedEntry.quantity);dailyTrades.push({entry_datetime:unclosedEntry.datetime,exit_datetime:last.datetime,entry_date:dayKey(unclosedEntry.datetime),exit_date:dayKey(last.datetime),entry_price:unclosedEntry.price,exit_price:mark,side:unclosedEntry.side,quantity:unclosedEntry.quantity,net:r.summary.open_pnl,closed:false});}
const markPrice=(t,e)=>t.entry_price+(e.equity-e.cash)/(t.side*r.config.point_value*t.quantity);
const equitySnapshots=[...r.equity].sort((a,b)=>a.datetime.localeCompare(b.datetime));
const snapshotBefore=key=>{const limit=`${key}T00:00:00`;let lo=0,hi=equitySnapshots.length;while(lo<hi){const m=(lo+hi)>>1;if(equitySnapshots[m].datetime<limit)lo=m+1;else hi=m;}return lo?equitySnapshots[lo-1]:null;};
const activeTradeAt=e=>{if(!e?.position)return null;let lo=0,hi=dailyTrades.length;while(lo<hi){const m=(lo+hi)>>1;if(dailyTrades[m].entry_datetime<=e.datetime)lo=m+1;else hi=m;}for(let i=lo-1;i>=0;i--){const t=dailyTrades[i];if(t.exit_datetime<e.datetime)break;if(t.entry_datetime<=e.datetime&&e.datetime<=t.exit_datetime&&t.side===e.position)return t;}return null;};
const firstPeriodDate=dailyTrades.reduce((v,t)=>!v||t.entry_date<v?t.entry_date:v,'');
const lastPeriodDate=r.equity.length?dayKey(equitySnapshots.at(-1).datetime):firstPeriodDate;
const dailyRows=[],dailyPnlByTrade=new Map();let priorEquity=r.config.initial_capital;
for(let date=firstPeriodDate;date&&date<=lastPeriodDate;date=addDays(date,1)){
  const current=equityByDay.get(date),previous=snapshotBefore(date),equity=current?current.equity:(previous?previous.equity:priorEquity),periodStartEquity=priorEquity;
  const active=dailyTrades.filter(t=>t.entry_date<=date&&t.exit_date>=date),contributions=[];
  for(const t of active){
    let pnl;const entryDay=t.entry_date===date,exitDay=t.exit_date===date;
    if(entryDay&&exitDay&&t.closed)pnl=t.net;
    else if(exitDay&&t.closed){const priorTrade=activeTradeAt(previous);const prevPrice=priorTrade?markPrice(priorTrade,previous):t.entry_price;pnl=(t.exit_price-prevPrice)*t.side*r.config.point_value*t.quantity-r.config.commission_per_side;}
    else if(entryDay){const price=current?.position&&t.entry_datetime<=current.datetime&&current.datetime<=t.exit_datetime?markPrice(t,current):t.entry_price;pnl=(price-t.entry_price)*t.side*r.config.point_value*t.quantity-r.config.commission_per_side;}
    else {const priorTrade=activeTradeAt(previous);const prevPrice=priorTrade?markPrice(priorTrade,previous):t.entry_price;const price=current?.position&&t.entry_datetime<=current.datetime&&current.datetime<=t.exit_datetime?markPrice(t,current):(t.closed&&t.exit_date<date?t.exit_price:prevPrice);pnl=(price-prevPrice)*t.side*r.config.point_value*t.quantity;}
    contributions.push([t,pnl]);const rows=dailyPnlByTrade.get(t)||new Map();rows.set(date,pnl);dailyPnlByTrade.set(t,rows);
  }
  const dailyPnl=equity-periodStartEquity,profit=contributions.reduce((s,[,v])=>s+Math.max(v,0),0),loss=contributions.reduce((s,[,v])=>s+Math.min(v,0),0),wins=contributions.filter(([,v])=>v>0).length;
  dailyRows.push({date,equity,pnl:dailyPnl,pct:periodStartEquity?dailyPnl/Math.abs(periodStartEquity)*100:0,profit,loss,trades:active,tradeCount:active.length,winRate:active.length?wins/active.length*100:0});priorEquity=equity;
}
const monthlyEquityPnl=new Map();for(const row of dailyRows){const key=row.date.slice(0,7);monthlyEquityPnl.set(key,(monthlyEquityPnl.get(key)||0)+row.pnl);}const monthlyEquityValues=[...monthlyEquityPnl.values()];put(analysis,'B31',mean(monthlyEquityValues));put(analysis,'B32',sampleSd(monthlyEquityValues));
const periodHeaders=[['期間','獲利(¤)','獲利(%)','毛利','毛損','交易次數','%勝率']];
section(periods,'A1','每日分析');matrix(periods,'A3',periodHeaders);header(periods,'A3:G3');
const dailyDisplayRows=mcDailyDisplayRows(dailyRows);
const dailyData=[...dailyDisplayRows].reverse().map(x=>[serialDay(x.date),x.pnl,x.pct,x.profit,x.loss,x.tradeCount,x.winRate]);matrix(periods,'A4',dailyData);
const dateIndex=new Map(dailyRows.map((x,i)=>[x.date,i]));
const makePrefix=key=>{const out=[0];for(const x of dailyRows)out.push(out.at(-1)+x[key]);return out;};
const prefix={pnl:makePrefix('pnl'),profit:makePrefix('profit'),loss:makePrefix('loss')};
const tradeSeries=new Map();for(const [t,rows]of dailyPnlByTrade){const entries=[...rows.entries()].sort(([a],[b])=>a.localeCompare(b)),sums=[0];for(const [,v]of entries)sums.push(sums.at(-1)+v);tradeSeries.set(t,{entries,sums});}
const tradeValueInRange=(t,start,end)=>{const s=tradeSeries.get(t);if(!s)return 0;let lo=0,hi=s.entries.length;while(lo<hi){const m=(lo+hi)>>1;if(s.entries[m][0]<start)lo=m+1;else hi=m;}const first=lo;lo=0;hi=s.entries.length;while(lo<hi){const m=(lo+hi)>>1;if(s.entries[m][0]<=end)lo=m+1;else hi=m;}return s.sums[lo]-s.sums[first];};
const tradeWindowStats=(start,end)=>{const selected=dailyTrades.filter(t=>t.entry_date<=end&&t.exit_date>=start);let wins=0,profit=0,loss=0;for(const t of selected){const value=tradeValueInRange(t,start,end);profit+=Math.max(value,0);loss+=Math.min(value,0);if(value>0)wins++;}return{selected,profit,loss,winRate:selected.length?wins/selected.length*100:0};};
const rangeRows=(start,end)=>{let first=dateIndex.get(start),last=dateIndex.get(end);if(first===undefined)first=dailyRows.findIndex(x=>x.date>start);if(first<0)first=dailyRows.length;if(last===undefined){last=dailyRows.length-1;while(last>=0&&dailyRows[last].date>end)last--;}if(first>=dailyRows.length||last<first)return{pnl:0,pct:0,profit:0,loss:0,count:0,winRate:0};const baseEquity=dailyRows[first-1]?.equity??r.config.initial_capital,pnl=prefix.pnl[last+1]-prefix.pnl[first],s=tradeWindowStats(start,end);return{pnl,pct:baseEquity?pnl/Math.abs(baseEquity)*100:0,profit:s.profit,loss:s.loss,count:s.selected.length,winRate:s.winRate};};
const dailySectionEnd=3+dailyDisplayRows.length,rollingTitleRow=dailySectionEnd+3,rollingHeaderRow=rollingTitleRow+2,rollingDataStart=rollingTitleRow+3,rollingDataEnd=rollingDataStart+dailyRows.length-1;
section(periods,`A${rollingTitleRow}`,'每日累計期間分析');matrix(periods,`A${rollingHeaderRow}`,periodHeaders);header(periods,`A${rollingHeaderRow}:G${rollingHeaderRow}`);
const dailyRolling=[...dailyRows].reverse().map((x,i)=>{const agg=rangeRows(x.date,lastPeriodDate);agg.label=i===0?`當日 - ${dateLabel(lastPeriodDate)}`:`${dateLabel(x.date)} - ${dateLabel(lastPeriodDate)}`;return[agg.label,agg.pnl,agg.pct,agg.profit,agg.loss,agg.count,agg.winRate];});matrix(periods,`A${rollingDataStart}`,dailyRolling);
const monthKeys=[];for(let k=firstPeriodDate.slice(0,7);k<=lastPeriodDate.slice(0,7);){monthKeys.push(k);const[y,m]=k.split('-').map(Number);k=m===12?`${y+1}-01`:`${y}-${String(m+1).padStart(2,'0')}`;}
const yearKeys=[...new Set(monthKeys.map(k=>k.slice(0,4)))];
const monthBounds=k=>{const[y,m]=k.split('-').map(Number),start=`${y}-${String(m).padStart(2,'0')}-01`,next=m===12?`${y+1}-01-01`:`${y}-${String(m+1).padStart(2,'0')}-01`;return[start,addDays(next,-1)];};
const yearBounds=y=>[`${y}-01-01`,`${y}-12-31`],formatMonth=k=>{const[y,m]=k.split('-');return`${Number(m)}/${y}`;};
const summaryTitleRow=rollingDataEnd+3;section(periods,`A${summaryTitleRow}`,'月分析');matrix(periods,`A${summaryTitleRow+2}`,[['期間','平均獲利','毛利','毛損','獲利因子','平均交易數量','%勝率']]);header(periods,`A${summaryTitleRow+2}:G${summaryTitleRow+2}`);
const monthNames=['一月','二月','三月','四月','五月','六月','七月','八月','九月','十月','十一月','十二月'];
const monthlyStatsByKey=new Map(monthKeys.map(k=>[k,rangeRows(...monthBounds(k))]));
const monthSummary=monthNames.map((name,i)=>{const stats=monthKeys.filter(k=>Number(k.slice(5,7))===i+1).map(k=>monthlyStatsByKey.get(k)),countMonths=stats.length,net=stats.reduce((s,x)=>s+x.pnl,0),profit=stats.reduce((s,x)=>s+Math.max(x.pnl,0),0),loss=stats.reduce((s,x)=>s+Math.min(x.pnl,0),0),tradeCount=stats.reduce((s,x)=>s+x.count,0),weightedWins=stats.reduce((s,x)=>s+x.count*x.winRate/100,0),average= countMonths?net/countMonths:0;return[name,average,profit,loss,loss?Math.sign(average)*profit/Math.abs(loss):0,countMonths?tradeCount/countMonths:0,tradeCount?weightedWins/tradeCount*100:0];});
matrix(periods,`A${summaryTitleRow+3}`,monthSummary);const monthSummaryEnd=summaryTitleRow+14;
const monthTitleRow=monthSummaryEnd+3;section(periods,`A${monthTitleRow}`,'每月分析');matrix(periods,`A${monthTitleRow+2}`,periodHeaders);header(periods,`A${monthTitleRow+2}:G${monthTitleRow+2}`);
const monthlyData=[...monthKeys].reverse().map(k=>{const[start]=monthBounds(k),v=monthlyStatsByKey.get(k);return[serialDay(start),v.pnl,v.pct,v.profit,v.loss,v.count,v.winRate];}),monthlyDataStart=monthTitleRow+3;matrix(periods,`A${monthlyDataStart}`,monthlyData);const monthlyDataEnd=monthlyDataStart+monthlyData.length-1;
const monthRollingTitle=monthlyDataEnd+3;section(periods,`A${monthRollingTitle}`,'每月累計期間分析');matrix(periods,`A${monthRollingTitle+2}`,periodHeaders);header(periods,`A${monthRollingTitle+2}:G${monthRollingTitle+2}`);
const monthRollingKeys=[...monthKeys].reverse(),monthlyRolling=monthRollingKeys.map((k,i)=>{const[start]=monthBounds(k),v=rangeRows(start,lastPeriodDate);return[i===0?`當日 - ${formatMonth(lastPeriodDate.slice(0,7))}`:`${formatMonth(k)} - ${formatMonth(lastPeriodDate.slice(0,7))}`,v.pnl,v.pct,v.profit,v.loss,v.count,v.winRate];}),monthlyRollingStart=monthRollingTitle+3;matrix(periods,`A${monthlyRollingStart}`,monthlyRolling);const monthlyRollingEnd=monthlyRollingStart+monthlyRolling.length-1;
const annualTitle=monthlyRollingEnd+3;section(periods,`A${annualTitle}`,'每年分析');matrix(periods,`A${annualTitle+2}`,periodHeaders);header(periods,`A${annualTitle+2}:G${annualTitle+2}`);
const annualData=[...yearKeys].reverse().map(y=>{const[start,end]=yearBounds(y),v=rangeRows(start,end);return[Number(y),v.pnl,v.pct,v.profit,v.loss,v.count,v.winRate];}),annualDataStart=annualTitle+3;matrix(periods,`A${annualDataStart}`,annualData);const annualDataEnd=annualDataStart+annualData.length-1;
const annualRollingTitle=annualDataEnd+3;section(periods,`A${annualRollingTitle}`,'每年累計期間分析');matrix(periods,`A${annualRollingTitle+2}`,periodHeaders);header(periods,`A${annualRollingTitle+2}:G${annualRollingTitle+2}`);
const annualRolling=[...yearKeys].reverse().map((y,i)=>{const[start]=yearBounds(y),v=rangeRows(start,lastPeriodDate);return[i===0?`當日 - ${lastPeriodDate.slice(0,4)}`:`${y} - ${lastPeriodDate.slice(0,4)}`,v.pnl,v.pct,v.profit,v.loss,v.count,v.winRate];}),annualRollingStart=annualRollingTitle+3;matrix(periods,`A${annualRollingStart}`,annualRolling);const annualRollingEnd=annualRollingStart+annualRolling.length-1;
periods.getRange(`B4:G${annualRollingEnd}`).format.horizontalAlignment='right';periods.getRange(`A4:A${dailySectionEnd}`).setNumberFormat('yyyy/mm/dd');periods.getRange(`A${monthlyDataStart}:A${monthlyDataEnd}`).setNumberFormat('yyyy/mm/dd');periods.getRange(`B4:G${annualRollingEnd}`).setNumberFormat('#,##0.00');
const chartContext={r,output,detail,periods,analysis,reportSettings,reportPath:input,mcLayout,put,section,monthlyStatsByKey,monthKeys,yearKeys,monthBounds,yearBounds,rangeRows,monthSummary,upper,lower,losingHeader,losingRunLengths,annualRollingEnd};
const chartLayout=await addMcReportPictures({...chartContext,simple,transport,allowAnalysisFallback:!!transport});
const settings=sheets['設定'];base(settings,'B',110+settingsOffset);settings.getRange('B:B').format.columnWidth=96;put(settings,'A1','設定');put(settings,'A3',`${strategyName} 研究版`);matrix(settings,'A4',inputRows);
const configRows=[[14,'商品名稱',r.provenance.symbol],[15,'原始資本',r.config.initial_capital],[16,'單邊佣金／口',r.config.commission_per_side],[17,'回測基本假設',r.config.intrabar_policy==='nearest_extreme'?'OHLC 先走距開盤較近的極值，等距先低':'兩條 OHLC 路徑；歧義棒選當棒收盤權益較差者'],[18,'滑價（跳）',r.config.slippage_ticks],[23,'壓縮',r.config.interval_minutes?`${r.config.interval_minutes} Minutes`:'日 K'],[24,'細部資料','本版彙整為 OHLC 後執行；非逐筆成交還原'],[25,'每點契約價值',r.config.point_value],[26,'資料開始（含暖機）',r.first_bar],[27,'資料結束',r.last_bar],[28,'回測模式',r.engine_version],[29,'K棒對齊位移（分鐘）',r.config.bar_anchor_minutes],[30,'時間標記／時區',`${r.config.timestamp_role} / ${r.provenance.timezone}`],[31,'暖機 K 棒數',r.config.warmup_bars],[32,'保證金模型',r.config.margin_per_contract??'未建模'],[33,'限制負權益新倉',r.config.enforce_cash_balance?'是':'否'],[34,'帳號驗證','依使用者指示停用研究版檢查；保留原始檔'],[35,'相容性狀態','尚未與 MC 逐筆驗證等價；未實作指標顯示 n/a'],[36,'期間損益口徑','依日曆日盯市及逐筆交易分攤；與 MC 逐項驗證中'],[37,'成交時間口徑','明細時間為 K 棒標記；棒內成交沒有真實逐筆時間，詳見 JSON fills.phase'],[38,'最大可能獲利／虧損','由選用的 OHLC 假設路徑計算，不代表真實逐筆 MFE／MAE'],[39,'研究假設',r.config.research_note],[40,'引擎雜湊',r.engine_sha256],[41,'設定雜湊',r.configuration_sha256],[42,'策略雜湊',r.strategy_sha256],[43,'行情雜湊',r.provenance.bars_sha256],[44,'委託價格取整',r.config.price_rounding??'directional'],[45,'Excel委託單編號','依成交順序編號；JSON 保留引擎內部 generated order_id，反向單平倉與進場各列一次']];
for(const[row,label,value]of configRows)matrix(settings,`A${row+settingsOffset}`,[[label,value]]);
if(r.config.price_rounding==='buy_floor_sell_ceil')put(settings,`B${44+settingsOffset}`,'買進／買回向下取整；賣出／放空向上取整');
const reportDate=value=>{if(!value)return null;const text=String(value),time=text.includes('T')?text.split('T')[1].slice(0,8):'00:00:00';return serialDay(text.slice(0,10))+excelTime(time);};
const mcSettingsRows=[[19,'利率',reportSettings.interest_rate],[20,'最低可接受報酬率',reportSettings.minimum_acceptable_return],[21,'投資者的風險趨避程度',reportSettings.risk_aversion],[22,'標準差倍數',reportSettings.standard_deviation_multiple],[23,'壓縮',reportSettings.interval_label],[24,'細部資料',reportSettings.detail_data],[25,'每點的契約價值',r.config.point_value],[26,'開始日期',reportDate(reportSettings.report_start)],[27,'結束日期',reportDate(reportSettings.report_end)],[28,'回測模式',reportSettings.backtest_mode],[29,'賣價資料序列',reportSettings.sell_price_series],[30,'買價資料序列',reportSettings.buy_price_series]].filter(([, ,v])=>v!==undefined&&v!==null);
for(const[row,label,value]of mcSettingsRows)matrix(settings,`A${row+settingsOffset}`,[[label,value]]);
section(settings,`A${47+settingsOffset}`,'研究限制');r.warnings.forEach((w,i)=>matrix(settings,`A${49+i+settingsOffset}`,[[i+1,w]]));settings.getRange(`B${17+settingsOffset}:B${110+settingsOffset}`).format.wrapText=true;settings.getRange(`A${35+settingsOffset}:B${110+settingsOffset}`).format.rowHeight=36;
settings.getRange(`A${39+settingsOffset}:B${39+settingsOffset}`).format.rowHeight=Math.max(96,Math.ceil(r.config.research_note.length/47)*14);
if(audit){section(settings,`A${75+settingsOffset}`,'本次策略原碼檢查（原樣保留）');audit.findings.forEach((f,i)=>{const row=77+settingsOffset+i;matrix(settings,`A${row}`,[[f.title,f.explanation]]);settings.getRange(`A${row}:B${row}`).format.rowHeight=62;});}
if(r.config.build_volume_on)matrix(settings,`A${46+settingsOffset}`,[['Build Volume On／Ticks口徑',r.config.build_volume_on==='trade_volume'?'Trade Volume：Ticks取當棒總成交股／口數；不是成交筆數':r.config.build_volume_on]]);
const observations=Object.entries(r.variable_observations||{}).filter(([n])=>['continecost','cost','entrylong','entryshort','vmark','value62'].includes(n));
if(observations.length){section(settings,`A${90+settingsOffset}`,'逐棒變數統計');observations.forEach(([name,v],i)=>matrix(settings,`A${92+settingsOffset+i}`,[[name,`計算${v.evaluated_bars}棒；非零${v.nonzero_bars}棒；最小${v.minimum??'n/a'}；最大${v.maximum??'n/a'}`]]));}
for(const [address,value]of Object.entries(reportMetrics.cells))put(analysis,address,value);
analysis.getRange('B39:B45').setNumberFormat('0.000000');analysis.getRange('B46:B54').setNumberFormat('0.00"%"');analysis.getRange('B28:D29').setNumberFormat('0.00"%"');analysis.getRange('B61').setNumberFormat('0.000000"%"');
analysis.getRange('B63:B64').setNumberFormat('yyyy/mm/dd');analysis.getRange('B65:B66').setNumberFormat('yyyy/mm/dd hh:mm:ss');
for(const address of reportMetrics.provisional_cells||[]){const row=Number(address.slice(1));analysis.mergeCells(`C${row}:D${row}`);put(analysis,`C${row}`,'待核對 MC 月回撤數列');analysis.getRange(`B${row}:D${row}`).format.fill='#FFF2CC';}
const layoutValidation=applyMcReportLayout({r,audit,mcLayout,sheets,put,matrix,section,settingsOffset,inputRows,reportSettings,reportMetrics,end,annualRollingEnd,dailySectionEnd,rollingTitleRow,rollingHeaderRow,rollingDataStart,rollingDataEnd,summaryTitleRow,monthTitleRow,monthlyDataStart,monthlyDataEnd,monthRollingTitle,monthlyRollingStart,monthlyRollingEnd,annualTitle,annualDataStart,annualDataEnd,annualRollingTitle,annualRollingStart,losingHeader,losingRunLengths,chartLayout,simple,transport});
if(formatRows>chartLayout.periodLastRow)periods.getRange(`A${chartLayout.periodLastRow+1}:G${formatRows}`).clear({applyTo:'all'});
await fs.writeFile(output.replace(/\.xlsx$/i,'.layout.json'),JSON.stringify({schema:'mc-export-layout-validation-v1',...chartLayout,...layoutValidation,periodTableHeaderRows:[1,rollingTitleRow,summaryTitleRow,monthTitleRow,monthRollingTitle,annualTitle,annualRollingTitle],reference_metrics_copied:false,profile_sha256:mcLayout.source_sha256},null,2));
await fs.writeFile(output.replace(/\.xlsx$/i,'.metrics.json'),JSON.stringify({formula_version:'mc-report-d2a9c655-v2',engine_sha256:r.engine_sha256,configuration_sha256:r.configuration_sha256,...reportMetrics},null,2));
wb.recalculate();
if(transport){
 const {writeTransportReport}=await import('./mc-report-transport.mjs');
 const table=(name,col,first,last,title=name)=>({name,title,first,rows:sheets[name].getRange(`A${first}:${col}${last}`).values});
 const settingsTable=table('設定','B',1,layoutValidation.settingsLastRow);
 const tables=simple?[
  table('策略分析','D',1,34,'策略績效總結果'),
  ...chartLayout.charts.map(chart=>({name:chart.section,title:chart.title,first:chart.row,rows:[[chart.title]]})),
  table('交易分析','D',1,20,'總交易分析'),
  table('週期性分析','G',monthTitleRow,monthlyDataEnd,'月週期分析'),
  table('週期性分析','G',annualTitle,annualDataEnd,'年週期分析'),settingsTable,
 ]:[table('策略分析','D',1,chartLayout.analysisLastRow),table('交易明細','P',1,3),table('交易分析','D',1,chartLayout.tradeLastRow),table('週期性分析','G',1,chartLayout.periodLastRow),settingsTable];
 if(!simple){
  r.trades.forEach((t,i)=>tables[1].rows.push(...tradeRows(t,i)));
  if(!r.trades.length&&r.open_trade)tables[1].rows.push([]);
  if(r.open_trade)tables[1].rows.push(...tradeRows(r.open_trade,r.trades.length,true));
 }
 const calculated=analysis.getRange('B4').values[0][0];
 if(typeof calculated!=='number'||Math.abs(calculated-r.summary.net_closed)>0.001)throw new Error('報表淨利對帳不符');
 const performance=performancePath?JSON.parse(await fs.readFile(performancePath,'utf8')):null;
 await writeTransportReport({output:output.replace(/\.xlsx$/i,`.${transport}`),format:transport,tables,performance,charts:chartLayout.charts,style,r,settingsDates:[65,66].map(row=>row-39+settingsOffset)});
 process.exit(0);
}
const computedCells={};for(const [name,lastCol,lastRow]of [['策略分析','D',chartLayout.analysisLastRow],['交易明細','P',end],['交易分析','D',chartLayout.tradeLastRow],['週期性分析','G',annualRollingEnd],['設定','B',layoutValidation.settingsLastRow]]){
 const values=sheets[name].getRange(`A1:${lastCol}${lastRow}`).values;computedCells[name]={};values.forEach((row,i)=>row.forEach((v,j)=>{const col=String.fromCharCode(65+j);if(typeof v==='number')computedCells[name][`${col}${i+1}`]=v;else if(v instanceof Date)computedCells[name][`${col}${i+1}`]=(v.getTime()-Date.UTC(1899,11,30))/86400000;}));
}
await fs.writeFile(output.replace(/\.xlsx$/i,'.computed-cells.json'),JSON.stringify(computedCells,null,2));
const overview=await wb.inspect({kind:'table',range:'策略分析!A3:D6',include:'values,formulas',tableMaxRows:4,tableMaxCols:4,maxChars:1500});console.log(overview.ndjson);
const calculated=analysis.getRange('B4').values[0][0];if(Math.abs(calculated-r.summary.net_closed)>0.001)throw new Error(`Excel 淨利對帳不符：${calculated} / ${r.summary.net_closed}`);
const errors=await wb.inspect({kind:'match',searchTerm:'#REF!|#DIV/0!|#VALUE!|#NAME\\?|#NUM!',options:{useRegex:true,maxResults:10},maxChars:1500});console.log(errors.ndjson);
await fs.mkdir(path.dirname(output),{recursive:true});
const previewPath=output.replace(/\.xlsx$/i,'.preview.png');
const chartPreviews=[['交易分析',`A${chartLayout.firstTradeChart}:F${chartLayout.firstTradeChart+39}`],['週期性分析',`A${chartLayout.firstPeriodChart}:G${chartLayout.firstPeriodChart+39}`]];
const previewRanges=process.env.REA_EXPORT_PREVIEW==='metrics'?[['策略分析','A37:D69']]:process.env.REA_EXPORT_PREVIEW==='charts'?chartPreviews:[['策略分析','A1:D34'],['策略分析','A37:D69'],['交易明細','A1:P9'],['交易分析','A1:D30'],['週期性分析','A1:G12'],['設定',`A1:B${69-39+settingsOffset}`],['設定',`A${layoutValidation.notesStart}:G${layoutValidation.notesStart+7}`],...chartPreviews];
if(chartLayout.analysisImageCount)previewRanges.push(['策略分析','A69:G120']);
const previews=[];if(process.env.REA_EXPORT_PREVIEW!=='0')for(const [name,range]of previewRanges){
  console.log(`繪製預覽：${name} ${range}`);
  const png=await wb.render({sheetName:name,range,scale:1.2,format:'png'});const p=previewPath.replace('.preview.png',`.${name}.${range.replace(':','-')}.preview.png`);await fs.writeFile(p,new Uint8Array(await png.arrayBuffer()));previews.push(p);
}
const xlsx=await SpreadsheetFile.exportXlsx(wb);await xlsx.save(output);
if(automaticOutput){await fs.mkdir(path.dirname(destination),{recursive:true});await fs.copyFile(output,destination,constants.COPYFILE_EXCL);}
console.log(JSON.stringify({output:destination,support_directory:path.dirname(output),previews,sheets:5,trades:r.trades.length}));
