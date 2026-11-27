// Native MC export presentation. Reference profile supplies labels/styles only;
// every numeric result below comes from this run's computed report.
import fs from 'node:fs/promises';
import path from 'node:path';
import {execFileSync} from 'node:child_process';
import {fileURLToPath} from 'node:url';

export function mcReportChartPlan(profile){
 const analysisCount=profile.analysis_chart_count??0,tradeCount=profile.trade_chart_count??2,periodCount=profile.period_chart_count??6;
 const limits=[10,11,7];
 for(const [i,count]of [analysisCount,tradeCount,periodCount].entries())if(!Number.isInteger(count)||count<0||count>limits[i])throw new Error('MC版面圖表數量超出已支援範圍');
 const tradeNames=['all-trades','winning-trades','losing-trades','mae','drawdown-pnl','mae-scatter','mae-scatter-pct','mfe','runup-pnl','mfe-scatter','mfe-scatter-pct'].slice(0,tradeCount);
 const periodNames=['monthly-pnl','monthly-pct','monthly-cumulative','monthly-average','annual-pnl','annual-pct','annual-cumulative'].slice(0,periodCount);
 return {analysisCount,tradeCount,periodCount,tradeNames,periodNames,full:analysisCount>0||tradeCount>2||periodCount>6};
}

export async function addMcReportPictures(c){
 const {r,output,detail,periods,analysis,reportSettings={},reportPath,mcLayout,put,section,monthlyStatsByKey,monthKeys,yearKeys,monthBounds,yearBounds,rangeRows,monthSummary,upper,lower,losingHeader,losingRunLengths,annualRollingEnd}=c;
 await fs.mkdir(path.dirname(output),{recursive:true});
 const firstTradeChart=losingHeader+losingRunLengths.length+3;
 const firstPeriodChart=annualRollingEnd+3;
 const python=path.join(path.dirname(path.dirname(process.execPath)),'..','python','python.exe');
 const renderer=fileURLToPath(new URL('./render-nh14-analysis-charts.py',import.meta.url));
 const analysisDirectory=path.join(path.dirname(output),'mc-analysis');
 if(c.simple){
  const charts=[7,8].map((number,i)=>({section:'策略分析',row:381+i*52,title:mcLayout.analysis_chart_titles[6+i],file:`mc-analysis/analysis-0${number}.png`}));
  if(c.transport==='html')execFileSync(python,[renderer,reportPath,'-',analysisDirectory,'--closed-only'],{encoding:'utf8',maxBuffer:1024*1024});
  return {firstTradeChart,firstPeriodChart,analysisLastRow:69,tradeLastRow:firstTradeChart,periodLastRow:annualRollingEnd,imageCount:c.transport==='html'?charts.length:0,charts};
 }
 const trades=[...r.trades,...(r.open_trade?[r.open_trade]:[])];
 const chartRow=(start,end,label,stats)=>{
  const selected=trades.filter(t=>t.entry_datetime.slice(0,10)<=end&&t.exit_datetime.slice(0,10)>=start);
  return {label,pnl:stats.pnl,profit:stats.profit,loss:stats.loss,count:stats.count,
   base:stats.pct?Math.abs(stats.pnl/stats.pct*100):r.config.initial_capital,
   mfe:selected.reduce((sum,t)=>sum+t.mfe,0),mae:selected.reduce((sum,t)=>sum+t.mae,0)};
 };
 const monthly=monthKeys.map(k=>chartRow(...monthBounds(k),k,monthlyStatsByKey.get(k)));
 const annual=yearKeys.map(k=>chartRow(...yearBounds(k),k,rangeRows(...yearBounds(k))));
 const input=output.replace(/\.xlsx$/i,'.chart-input.json');
 const {analysisCount:plannedAnalysisCount,tradeCount,periodCount,tradeNames,periodNames,full}=mcReportChartPlan(mcLayout);
 let analysisCount=plannedAnalysisCount;
 await fs.writeFile(input,JSON.stringify({full_mc_layout:full,point_value:r.config.point_value,trades:r.trades,open_trade:r.open_trade,outlier_upper:Number.isFinite(upper)?upper:null,outlier_lower:Number.isFinite(lower)?lower:null,monthly,annual,average_months:monthSummary.map(row=>({label:row[0],pnl:row[1],count:1}))}));
 const result=execFileSync(python,[fileURLToPath(new URL('./render-mc-report-charts.py',import.meta.url)),input],{encoding:'utf8',maxBuffer:1024*1024});
 console.log(result.trim());
 const charts=[];
 const picture=async(s,sheet,title,file,row)=>{
  charts.push({section:sheet,row,title,file});
  if(!c.transport){
   const raw=await fs.readFile(path.join(path.dirname(output),file));
   s.images.add({dataUrl:'data:image/png;base64,'+raw.toString('base64'),anchor:{from:{row:row+1,col:0},extent:{widthPx:800,heightPx:800}}});
  }
 };
 for(let i=0;i<mcLayout.trade_chart_titles.length;i++){
  const row=firstTradeChart+i*52;section(detail,`A${row}`,mcLayout.trade_chart_titles[i]);
  if(i<tradeNames.length)await picture(detail,'交易分析',mcLayout.trade_chart_titles[i],`mc-charts/${tradeNames[i]}.png`,row);
 }
 for(let i=0;i<mcLayout.period_chart_titles.length;i++){
  const row=firstPeriodChart+i*52;section(periods,`A${row}`,mcLayout.period_chart_titles[i]);
  if(i<periodNames.length)await picture(periods,'週期性分析',mcLayout.period_chart_titles[i],`mc-charts/${periodNames[i]}.png`,row);
 }
 let analysisReceipt=null;
 if(analysisCount){
  const dir=analysisDirectory;
  try {
   if(!analysis||!reportPath||!reportSettings.analysis_bars_path)throw new Error('策略績效圖片需要本次引擎報告及同一次運算的行情');
   console.log(execFileSync(python,[renderer,reportPath,reportSettings.analysis_bars_path,dir,...(c.transport?['--transport']:[])],{encoding:'utf8',maxBuffer:1024*1024}).trim());
   analysisReceipt=JSON.parse(await fs.readFile(path.join(dir,'receipt.json'),'utf8'));
  } catch(error) {
   if(!c.allowAnalysisFallback)throw error;
   console.warn(error.message);
   analysisCount=0;
   reportSettings.validation_note+=' 策略分析逐價重播未通過核對，相關圖片及依賴重播的側別數值未列入。';
  }
  if(analysisReceipt){
  // 交易MFE與MAE不能假設先後；使用逐價帳本重播的側別回撤。
  if(analysisReceipt.side_max_drawdown)for(const [side,col]of [['long','C'],['short','D']]){
   const drawdown=analysisReceipt.side_max_drawdown?.[side];
   if(!Number.isFinite(drawdown))throw new Error('缺少已核對的多空逐價回撤');
   put(analysis,`${col}16`,drawdown);
   put(analysis,`${col}17`,analysisReceipt.side_max_drawdown_percent[side]);
   const sideNet=r.trades.filter(t=>t.side===(side==='long'?1:-1)).reduce((sum,t)=>sum+t.net,0);
   put(analysis,`${col}20`,drawdown<0?sideNet/(-drawdown):'n/a');
  }
  for(const [side,col]of [['all','B'],['long','C'],['short','D']])put(analysis,`${col}30`,analysisReceipt.buy_hold[side]);
  }
  for(let i=0;i<plannedAnalysisCount;i++){
   const row=69+i*52,title=mcLayout.analysis_chart_titles[i];
   section(analysis,`A${row}`,title);
   const file=`analysis-${String(i+1).padStart(2,'0')}.png`;
   if(analysisReceipt?.images.some(item=>item.file===file))await picture(analysis,'策略分析',title,`mc-analysis/${file}`,row);
  }
  analysisCount=analysisReceipt?.images.filter(item=>Number(item.file.slice(9,11))<=plannedAnalysisCount).length??0;
 }
 return {firstTradeChart,firstPeriodChart,analysisLastRow:69+Math.max(plannedAnalysisCount-1,0)*52,tradeLastRow:firstTradeChart+Math.max(mcLayout.trade_chart_titles.length-1,0)*52,periodLastRow:periodCount?firstPeriodChart+Math.max(mcLayout.period_chart_titles.length-1,0)*52:annualRollingEnd,imageCount:charts.length,analysisImageCount:analysisCount,tradeImageCount:tradeNames.length,periodImageCount:periodNames.length,profile_path:reportSettings.layout_profile||'data/mc-report-layout.json',analysisReceipt,charts};
}

export function applyMcReportLayout(c){
 const {r,audit,mcLayout,sheets,put,matrix,section,settingsOffset,inputRows,reportSettings,reportMetrics,end,annualRollingEnd,dailySectionEnd,rollingTitleRow,rollingHeaderRow,rollingDataStart,rollingDataEnd,summaryTitleRow,monthTitleRow,monthlyDataStart,monthlyDataEnd,monthRollingTitle,monthlyRollingStart,monthlyRollingEnd,annualTitle,annualDataStart,annualDataEnd,annualRollingTitle,annualRollingStart,losingHeader,losingRunLengths,chartLayout}=c;
 const {策略分析:analysis,交易明細:trades,交易分析:detail,週期性分析:periods,設定:settings}=sheets;
 const rangeStyle=(s,a)=>{s.getRange(a).setNumberFormat('General');s.getRange(a).format.font={name:mcLayout.font,size:12};s.getRange(a).format.rowHeight=mcLayout.row_height;s.getRange(a).format.wrapText=false;};
 rangeStyle(analysis,`A1:D${chartLayout.analysisLastRow}`);rangeStyle(trades,`A1:P${c.transport?3:end}`);rangeStyle(detail,`A1:D${chartLayout.tradeLastRow}`);rangeStyle(periods,`A1:G${chartLayout.periodLastRow}`);
 for(const [name,labels]of Object.entries(mcLayout.labels))for(const [a,text]of Object.entries(labels)){
  const address=name==='設定'?'A'+(Number(a.slice(1))-39+settingsOffset):a;
  put(sheets[name],address,text);sheets[name].getRange(address).format.font={name:mcLayout.font,size:12,bold:true};
 }
 for(const s of Object.values(sheets)){s.getRange('A1').format.font={name:mcLayout.font,size:14.4,bold:true};s.getRange('A1').format.rowHeight=20.6;}
 if(!c.transport){
 trades.getRange(`E4:E${end}`).setNumberFormat('yyyy/mm/dd');trades.getRange(`F4:F${end}`).setNumberFormat('hh:mm:ss');
 }
 trades.getRange('A3:P3').format.font={name:mcLayout.font,size:12,bold:true};
 const duration=s=>String(s).replace(/(\d+) 年/g,'$1 Yrs,').replace(/(\d+) 個月/g,'$1 Mths,').replace(/(\d+) 天/g,'$1 Dys,').replace(/(\d+) 小時/g,'$1 Hrs,').replace(/(\d+) 分鐘/g,'$1 Mins').replace(/,\s*$/,'');
 for(const a of ['B59','B60','B62'])if(analysis.getRange(a).values[0][0]!=null){const value=duration(analysis.getRange(a).values[0][0]);put(analysis,a,value);if(a in reportMetrics.cells)reportMetrics.cells[a]=value;}
 analysis.getRange('B63:B64').setNumberFormat('yyyy/mm/dd');analysis.getRange('B65:B66').setNumberFormat('yyyy/mm/dd hh:mm:ss');
 detail.getRange('B37:C37').setNumberFormat('yyyy/mm/dd');detail.getRange('B40:C40').setNumberFormat('yyyy/mm/dd');
 const periodTitles=[1,rollingTitleRow,summaryTitleRow,monthTitleRow,monthRollingTitle,annualTitle,annualRollingTitle];
 periodTitles.forEach((row,i)=>section(periods,`A${row}`,mcLayout.period_titles[i]));
 for(const row of [3,rollingHeaderRow,summaryTitleRow+2,monthTitleRow+2,monthRollingTitle+2,annualTitle+2,annualRollingTitle+2])periods.getRange(`A${row}:G${row}`).format.font={name:mcLayout.font,size:12,bold:true};
 periods.getRange(`A4:A${dailySectionEnd}`).setNumberFormat('yyyy/mm/dd');periods.getRange(`A${monthlyDataStart}:A${monthlyDataEnd}`).setNumberFormat('yyyy/mm/dd');
 if(!c.simple){
  for(let i=0;i<mcLayout.trade_chart_titles.length;i++)section(detail,`A${chartLayout.firstTradeChart+i*52}`,mcLayout.trade_chart_titles[i]);
  for(let i=0;i<mcLayout.period_chart_titles.length;i++)section(periods,`A${chartLayout.firstPeriodChart+i*52}`,mcLayout.period_chart_titles[i]);
 }
 // Native settings occupy the same input-dependent rows. Explanations begin
 // only after that block, never in metric tables or chart captions.
 settings.getRange(`A1:B${149+settingsOffset}`).values=Array.from({length:149+settingsOffset},()=>[null,null]);
 settings.getRange('A:B').format.columnWidth=mcLayout.value_column_width;settings.getRange('A:A').format.columnWidth=mcLayout.label_column_width;
 rangeStyle(settings,`A1:B${149+settingsOffset}`);put(settings,'A1','設定');put(settings,'A3',r.config.strategy_name);matrix(settings,'A4',inputRows);
 for(const [a,text]of Object.entries(mcLayout.labels['設定']))put(settings,'A'+(Number(a.slice(1))-39+settingsOffset),text);
 const offset=39-settingsOffset;
 const date=s=>{const t=String(s||'');return t?(Date.parse(t.endsWith('Z')?t:t+'Z')-Date.UTC(1899,11,30))/86400000:null;};
 const actualStart=r.trades[0]?.entry_datetime||r.first_bar;
 const values={53:r.provenance.symbol,54:r.config.initial_capital,55:`凱基單邊${r.config.commission_per_side} [固定式${r.config.commission_per_side}¤ 每口]`,56:'當價格觸及限價或穿價時，限價單完全成交 (傳統模式)',57:`${r.config.slippage_ticks*r.config.tick_size*r.config.point_value}¤ 每筆`,58:reportSettings.interest_rate??'n/a',59:reportSettings.minimum_acceptable_return??'n/a',60:reportSettings.risk_aversion??'n/a',61:reportSettings.standard_deviation_multiple??'n/a',62:`${r.config.interval_minutes*60} Seconds`,63:'已停用。',64:r.config.point_value,65:date(actualStart),66:date(r.last_bar),67:r.tick_execution?'逐筆 Tick 撮合':'傳統模式',68:r.provenance.symbol,69:r.provenance.symbol};
 for(const [row,value]of Object.entries(values))put(settings,'B'+(Number(row)-offset),value);
 settings.getRange(`B${65-offset}:B${66-offset}`).setNumberFormat('yyyy/mm/dd hh:mm:ss');
 settings.getRange(`A1:A${69-offset}`).format.font={name:mcLayout.font,size:12,bold:true};settings.getRange('A1').format.font={name:mcLayout.font,size:14.4,bold:true};
 const explanations=[['報告來源',`自製引擎 ${r.engine_version}；${r.config.strategy_name}。數字由本次回測計算，未複製原始 MC 績效。`],['相容性','本次只對齊 MC Excel 的欄位、文字及配置；交易、指標數值與圖表像素尚未證明完全等價。'],['回測設定',r.config.research_note],['委託價格取整',r.config.price_rounding||'directional'],['MaxBarsBack',r.config.warmup_bars],['資料範圍',`${r.first_bar} 至 ${r.last_bar}，含暖機`],['報告版面範本',mcLayout.source],['版面範本雜湊',mcLayout.source_sha256],['引擎雜湊',r.engine_sha256],['設定雜湊',r.configuration_sha256],['策略雜湊',r.strategy_sha256],['行情雜湊',r.provenance.bars_sha256],['圖表口徑','依原版輸出八張圖；圖片均以本次引擎資料重新繪製。期間最大可能獲利／虧損為該期間重疊交易之整筆 MFE／MAE 加總，尚未與 MC 原生期間圖表驗證。'],['每日表','每日分析列有交易或持倉關聯的日曆日，含持倉跨越的無行情日；無新行情時沿用前一權益，不補造行情棒。'],['月指標驗證',reportMetrics.monthly_risk_validation],...r.warnings.map((text,i)=>[`研究限制 ${i+1}`,text])];
 const firstNote=72-offset;section(settings,`A${firstNote}`,'必要說明');settings.getRange('C:G').format.columnWidth=mcLayout.value_column_width;
 if(reportSettings.validation_note)explanations.push(['逐筆比對',reportSettings.validation_note]);
 const riskLabels={interest_rate:'年利率',minimum_acceptable_return:'年最低可接受報酬率',risk_aversion:'風險趨避程度'};
 if(reportMetrics.defaulted_risk_settings.length)explanations.push(['績效比率預設值',reportMetrics.defaulted_risk_settings.map(key=>riskLabels[key]).join('、')+'未在任務指定，採用本專案 MC 參考報告的設定。上述利率及報酬率以百分數表示。']);
 explanations.push(['交易分析 K 棒數','持有棒數含進場與出場棒，同棒進出為 1 棒。MC 本版本的平倉交易間欄取扣費後損益兩平交易的平均持有棒數。獲利及虧損交易間欄各用總 K 棒數扣除該類交易的持有棒數合計，再除以該類交易筆數。']);
 if(c.transport)explanations.push(['原生欄位差異','Z 值及信賴界限尚缺原生口徑。部分風險比率由任務提供的利率與風險設定計算。最大可能獲利／虧損日期目前按對應交易日期取值，可能與 MC 的棒內極值日期不同。']);
 explanations.find(row=>row[0]==='圖表口徑')[1]=`依原始 MC 報告配置 ${chartLayout.imageCount} 張圖；均由本次引擎資料重繪，未複製原版績效圖片。期間 MFE／MAE 為重疊交易的整筆數值加總，尚未驗證原生期間圖表等價。`;
 if(chartLayout.tradeImageCount>2){
  explanations.push(['交易圖口徑','虧損的損益畫已平倉交易的 MAE 至淨損益，獲利的損益畫淨損益至 MFE，按最終盈虧著色。最大可能虧損與獲利點圖納入期末未平倉交易。幅度散佈圖使用已平倉交易的 MAE／MFE 與淨損益絕對值，百分比除以進場名目金額。刻度依實際數值範圍配置。']);
 }
 if(chartLayout.analysisReceipt){
  for(const text of chartLayout.analysisReceipt.limitations)explanations.push(['策略圖公式',text]);
 }
 if(audit?.findings)for(const finding of audit.findings)explanations.push([finding.title,finding.explanation]);
 explanations.forEach(([label,text],i)=>{const row=firstNote+2+i;matrix(settings,`A${row}`,[[label,text]]);settings.mergeCells(`B${row}:G${row}`);settings.getRange(`B${row}:G${row}`).format.wrapText=true;settings.getRange(`A${row}:G${row}`).format.verticalAlignment='top';settings.getRange(`A${row}:G${row}`).format.rowHeight=Math.max(33.5,(Math.ceil(String(text).length/45)+1)*16.75);});
 analysis.getRange('A1').format.font={name:mcLayout.font,size:14.4,bold:true};periods.getRange('A1').format.font={name:mcLayout.font,size:14.4,bold:true};
 return {settingsLastRow:firstNote+1+explanations.length,notesStart:firstNote,notesOnlyInSettings:true};
}
