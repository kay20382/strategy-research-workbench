// 原生MC參考的多空欄位證明：獲利因子符號依該母體淨利，非多空方向。
export function mcProfitFactor(profit,loss){
 if(!(loss<0))return 'n/a';
 return Math.sign(profit+loss)*profit/(-loss);
}
export function mcRunLengths(runs,sign){
 return [...new Set(runs.filter(x=>x.sign===sign).map(x=>x.items.length))].sort((a,b)=>a-b);
}
// ReportEngine uses inclusive holding bars. The three "between" rows use
// FlatTradeIndex, ProfitTradeIndex, and LossTradeIndex respectively.
export function mcTradeBarAverages(trades,barCount){
 const winners=trades.filter(t=>t.net>0),losers=trades.filter(t=>t.net<0);
 const flat=trades.filter(t=>Math.abs(t.net)<1e-8);
 const held=group=>group.reduce((sum,t)=>sum+t.exit_bar-t.entry_bar+1,0);
 const average=(total,count)=>{
  if(!count)return 'n/a';
  const value=total/count;
  return Math.sign(value)*Math.round(Math.abs(value)*10)/10;
 };
 return [average(held(trades),trades.length),average(held(winners),winners.length),
  average(held(losers),losers.length),average(held(flat),flat.length),
  average(barCount-held(winners),winners.length),average(barCount-held(losers),losers.length)];
}
// MC每日表列有交易關聯的日曆日，含持倉跨過的無行情日；不新增行情棒。
export function mcDailyDisplayRows(rows){return rows.filter(x=>x.tradeCount>0);}
export function mcClosedDrawdownPercent(trades,initial){
 if(!trades.length)return 'n/a';
 let equity=initial,peak=initial,lowest=0;
 for(const t of trades){equity+=t.net;peak=Math.max(peak,equity);if(peak>0)lowest=Math.min(lowest,100*(equity-peak)/peak);}
 return lowest;
}
export function mcRunPercents(trades,initial){
 let equity=initial,sign=0,opening=initial,pnl=0,winning=0,losing=0;
 const finish=()=>{if(opening>0){const pct=100*pnl/opening;if(sign>0)winning=Math.max(winning,pct);if(sign<0)losing=Math.min(losing,pct);}};
 for(const t of trades){const next=Math.sign(t.net);if(next!==sign||!next){finish();sign=next;opening=equity;pnl=0;}pnl+=t.net;equity+=t.net;}
 finish();return {winning,losing};
}
