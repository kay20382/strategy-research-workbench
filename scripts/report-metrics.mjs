// Pure calculations: engine JSON and report settings only. MC reference workbooks
// belong to the independent comparator and are never inputs to these formulas.
const DAY=86400000;
const ms=value=>Date.parse(/[zZ]|[+-]\d\d:\d\d$/.test(value)?value:`${value}Z`);
const mean=xs=>xs.length?xs.reduce((a,b)=>a+b,0)/xs.length:0;
const sd=(xs,sample)=>xs.length>(sample?1:0)?Math.sqrt(xs.reduce((a,b)=>a+(b-mean(xs))**2,0)/(xs.length-(sample?1:0))):0;
const finite=value=>Number.isFinite(value)?value:'n/a';
const serial=value=>(ms(value)-Date.UTC(1899,11,30))/DAY;
const riskDefaults={interest_rate:2,minimum_acceptable_return:2,risk_aversion:'保守'};
const reportNumber=value=>typeof value==='number'||(typeof value==='string'&&value.trim()!=='')?Number(value):NaN;

function resolveRiskSettings(settings){
  const risk_settings={}, defaulted_risk_settings=[];
  for(const [key,fallback] of Object.entries(riskDefaults)){
    const supplied=key==='risk_aversion'&&Number.isFinite(settings.risk_aversion_coefficient)
      ?settings.risk_aversion_coefficient:settings[key];
    if(supplied==null||(typeof supplied==='string'&&supplied.trim()==='')){
      risk_settings[key]=fallback;
      defaulted_risk_settings.push(key);
    }else{
      risk_settings[key]=supplied;
    }
  }
  return {risk_settings,defaulted_risk_settings};
}

// ReportEngine.dll FUN_1800e6f80 uses 364.25 days/year and 30 days/month.
// Preserve its component counts, translating the displayed units to Chinese.
export function mcDuration(days){
  if(!Number.isFinite(days)||days<0)return 'n/a';
  const rounded=days+0.5/86400,whole=Math.trunc(rounded),years=Math.trunc(whole/364.25);
  const remaining=whole+Math.trunc(-years*364.25);
  const parts=[[years,'年'],[Math.trunc(remaining/30),'個月'],[remaining%30,'天'],[Math.trunc(rounded*24)%24,'小時'],[Math.trunc(rounded*1440)%60,'分鐘'],[Math.trunc(rounded*86400)%60,'秒']];
  return parts.filter(([v])=>v).map(([v,u])=>`${v} ${u}`).join(' ')||'0 分鐘';
}

// Native helper FUN_1800e75e0 changes the sign according to magnitudes.
export function mcMagnitudeRatio(numerator,denominator){
  const d=Math.abs(denominator);
  return d?Math.abs(numerator)/d*(Math.abs(numerator)>=d?1:-1):0;
}

export function computeMcReportMetrics(r,settings={}){
  const cells={},equity=r.equity,capital=r.config.initial_capital;
  const effective=resolveRiskSettings(settings),risk=effective.risk_settings;
  const validStart=equity[Math.min(r.config.warmup_bars,equity.length-1)]?.datetime;
  const end=equity.at(-1)?.datetime;
  if(!validStart||!end)return {cells,monthly:[],status:'沒有可用權益資料',...effective};
  const periodDays=(ms(end)-ms(validStart))/DAY;
  const allTrades=[...r.trades,...(r.open_trade?[r.open_trade]:[])];
  const spans=allTrades.map(t=>[Math.max(ms(validStart),ms(t.entry_datetime)),Math.min(ms(end),ms(t.exit_datetime))]).filter(([a,b])=>b>=a).sort((a,b)=>a[0]-b[0]);
  const merged=[];
  for(const [a,b]of spans){if(merged.length&&a<=merged.at(-1)[1])merged.at(-1)[1]=Math.max(b,merged.at(-1)[1]);else merged.push([a,b]);}
  const occupied=merged.reduce((sum,[a,b])=>sum+b-a,0)/DAY;
  const flat=merged.slice(1).reduce((best,x,i)=>Math.max(best,x[0]-merged[i][1]),0)/DAY;
  Object.assign(cells,{B59:mcDuration(periodDays),B60:mcDuration(occupied),B61:periodDays?occupied/periodDays*100:'n/a',B62:mcDuration(flat)});
  if(allTrades.length){
    const mfe=allTrades.reduce((a,b)=>b.mfe>a.mfe?b:a),mae=allTrades.reduce((a,b)=>b.mae<a.mae?b:a);
    cells.B63=serial(`${mfe.exit_datetime.slice(0,10)}T00:00:00`);
    cells.B64=serial(`${mae.exit_datetime.slice(0,10)}T00:00:00`);
  }
  if(equity.every(e=>Number.isFinite(e.bar_drawdown))){
    const worst=equity.reduce((a,b)=>b.bar_drawdown<a.bar_drawdown?b:a);
    cells.B65=serial(worst.datetime);
  }
  let peak=0,worst=0,worstTrade=null;
  for(const t of r.trades){peak=Math.max(peak,t.cumulative);if(t.cumulative-peak<worst){worst=t.cumulative-peak;worstTrade=t;}}
  if(worstTrade)cells.B66=serial(worstTrade.exit_datetime);
  const worstNet=Math.min(0,...r.trades.map(t=>t.net));
  const worstMae=Math.min(0,...allTrades.map(t=>t.mae));
  const groups=[r.trades,r.trades.filter(t=>t.side===1),r.trades.filter(t=>t.side===-1)];
  for(let i=0;i<groups.length;i++){
    const net=groups[i].reduce((sum,t)=>sum+t.net,0),col=['B','C','D'][i];
    cells[`${col}28`]=periodDays&&capital?net/capital*100*365.25/periodDays:'n/a';
    cells[`${col}29`]=typeof cells[`${col}28`]==='number'?cells[`${col}28`]/12:'n/a';
  }
  const win=r.trades.filter(t=>t.net>0),loss=r.trades.filter(t=>t.net<0);
  const net=r.summary.net_closed;
  const adjusted=win.reduce((s,t)=>s+t.net,0)*(win.length?1-1/Math.sqrt(win.length):0)+loss.reduce((s,t)=>s+t.net,0)*(loss.length?1+1/Math.sqrt(loss.length):0);
  const winMean=mean(win.map(t=>t.net)),lossMean=mean(loss.map(t=>t.net));
  const upper=winMean+sd(win.map(t=>t.net),true),lower=lossMean-sd(loss.map(t=>t.net),true);
  const selected=r.trades.filter(t=>t.net<=upper&&t.net>=lower).reduce((s,t)=>s+t.net,0);
  [net,selected,adjusted].forEach((n,j)=>[worstNet,worstMae,r.summary.max_drawdown].forEach((d,k)=>{cells[`B${46+j*3+k}`]=d?n/Math.abs(d)*100:'n/a';}));

  // Calendar months with no bars remain present and carry the last equity.
  const firstMonth=(settings.report_start||r.trades[0]?.entry_datetime||validStart).slice(0,7),lastMonth=end.slice(0,7);
  const byMonth=new Map();for(const e of equity)byMonth.set(e.datetime.slice(0,7),e.equity);
  const monthly=[];let previous=capital;
  for(let key=firstMonth;key<=lastMonth;){
    const value=byMonth.get(key)??previous,pnl=value-previous;
    monthly.push({month:key,opening_equity:previous,closing_equity:value,pnl,return:previous?pnl/Math.abs(previous):0});previous=value;
    const [y,m]=key.split('-').map(Number);key=m===12?`${y+1}-01`:`${y}-${String(m+1).padStart(2,'0')}`;
  }
  const returns=monthly.map(x=>x.return),avg=mean(returns),deviation=sd(returns,true);
  const interest=reportNumber(risk.interest_rate)/100,mar=reportNumber(risk.minimum_acceptable_return)/100,target=mar/12;
  cells.B40=finite((avg-interest/12)/deviation);cells.B41=typeof cells.B40==='number'?cells.B40*Math.sqrt(12):'n/a';
  // The installed MC build uses the population SD of the below-target subset,
  // rather than the common lower-partial-moment downside-deviation formula.
  const below=returns.filter(x=>x<target),downside=sd(below,false);
  cells.B42=finite((avg-target)/downside);
  const aversion=risk.risk_aversion==='保守'?3:reportNumber(risk.risk_aversion);
  cells.B43=returns.length&&Number.isFinite(target)?finite(avg-aversion*downside**2):'n/a';
  const up=returns.reduce((s,x)=>s+Math.max(x-target,0),0),down=returns.reduce((s,x)=>s+Math.min(x-target,0)**2,0);
  // MC uses the sum of squared shortfalls directly; it does not take its root.
  cells.B39=returns.length>=36&&Number.isFinite(target)?finite(mcMagnitudeRatio(up,down)):'n/a';

  const hasIntrabar=equity.every(e=>Number.isFinite(e.equity_low)&&Number.isFinite(e.equity_low_after_high));
  if(hasIntrabar&&monthly.length){
    // CEquityCalculator seeds detailed fill bars with MinEquityAfterMax in
    // both slots 2 and 3 (FUN_180133a50). Native EquityClose reads slot 3;
    // it therefore differs from marked-to-market closing equity on fill bars.
    const fillBars=new Set([...allTrades.map(t=>t.entry_bar),...r.trades.map(t=>t.exit_bar)]);
    const nativeClose=i=>fillBars.has(i)?equity[i].equity_low_after_high:equity[i].equity;
    const bounds=new Map();for(const [i,e]of equity.entries()){const key=e.datetime.slice(0,7);if(!bounds.has(key))bounds.set(key,[i,i]);else bounds.get(key)[1]=i;}
    for(const month of monthly){
      const active=allTrades.filter(t=>t.entry_datetime.slice(0,7)<=month.month&&t.exit_datetime.slice(0,7)>=month.month);
      let bound=bounds.get(month.month);
      if(!bound){const next=equity.findIndex(e=>e.datetime.slice(0,7)>month.month);if(next>=0)bound=[next,next-1];}
      const ranges=[];
      for(const t of active){
        if(!bound)continue;
        const a=Math.max(t.entry_bar,bound[0]),b=Math.min(t.exit_bar,bound[1]);
        const overlap=ranges.find(x=>a<x[1]&&x[0]<b);
        if(overlap){overlap[0]=Math.min(overlap[0],a);overlap[1]=Math.max(overlap[1],b);}else ranges.push([a,b]);
      }
      let drawdown=0;
      const riskRanges=[];
      for(const [a,b]of ranges){
        // Native min_element returns begin even for the synthetic empty
        // interval in a missing calendar month; its dereference uses the
        // first available following bar. Preserve this observed convention.
        let low=equity[a].equity_low;for(let i=a+1;i<=b;i++)low=Math.min(low,equity[i].equity_low);
        const before=a>allTrades[0]?.entry_bar?nativeClose(a-1):capital;
        riskRanges.push({start_bar:a,end_bar:b,before_equity:before,minimum_equity:low});
        drawdown+=Math.min(0,low-before);
      }
      drawdown=Math.min(drawdown,month.pnl,0);
      month.risk_ranges=riskRanges;
      month.drawdown=drawdown;month.drawdown_fraction=month.opening_equity?drawdown/month.opening_equity:0;
    }
    const last=monthly.slice(-36).map(x=>x.drawdown_fraction),minimum=Math.min(...last);
    cells.B44=minimum?finite((mar/12)/Math.abs(minimum)):'n/a';
    // The native iterator takes up to three 12-month blocks from the oldest end.
    const yearlyMin=[];for(let i=0;i<Math.min(monthly.length,36);i+=12)yearlyMin.push(Math.min(...monthly.slice(i,i+12).map(x=>x.drawdown_fraction)));
    cells.B45=finite(mar/Math.abs(mean(yearlyMin)-10));
  }
  return {cells,monthly,...effective,period_start:validStart,period_end:end,period_days:periodDays,occupied_days:occupied,downside_deviation:downside,risk_aversion_coefficient:aversion,has_intrabar:hasIntrabar,provisional_cells:[],monthly_risk_validation:hasIntrabar?'使用棒內最後高點後低點與月界合成持倉計算；KAIv7 標準案例另以原生 80 個月份逐區間核對':'缺少棒內權益低點或最後高點後低點，未計算月回撤比率'};
}
