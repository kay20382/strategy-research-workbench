import fs from 'node:fs/promises';
import path from 'node:path';

const columns = [...'ABCDEFGHIJKLMNOP'];
const tradeMerge = new Set([0,8,9,10,11,12,13,14,15]);
const escape = value => String(value ?? '').replace(/[&<>"']/g, c => ({
  '&':'&amp;', '<':'&lt;', '>':'&gt;', '"':'&quot;', "'":'&#39;'
})[c]);
const nonempty = row => row.some(value => value !== null && value !== undefined && value !== '');
const normalize = value => value instanceof Date ? (value.getTime()-Date.UTC(1899,11,30))/86400000 : value;
const amount = new Intl.NumberFormat('en-US',{minimumFractionDigits:2,maximumFractionDigits:2});
const integer = new Intl.NumberFormat('en-US',{maximumFractionDigits:0});
const riskRatio = new Intl.NumberFormat('en-US',{minimumFractionDigits:6,maximumFractionDigits:6});
function csvCell(value) {
  const text = typeof value === 'string' && /^[\s]*[=+\-@]/.test(value) ? "'" + value : String(value ?? '');
  return '"' + text.replaceAll('"','""') + '"';
}
function htmlCell(section, row, column, value, settingsDates, header, rowspan, label) {
  const v=normalize(value);
  const tag=header?'th':'td';
  let rendered=v, negative=false;
  if(typeof v==='number'){
    const trades=section==='交易明細'&&row>=4;
    const period=section==='週期性分析';
    const analysisDate=section==='策略分析'&&column===1&&row>=63&&row<=66;
    const settingsDate=section==='設定'&&column===1&&settingsDates.includes(row);
    if(trades&&column===5){
      const seconds=Math.round(v*86400);
      rendered=[Math.floor(seconds/3600),Math.floor(seconds/60)%60,seconds%60].map(n=>String(n).padStart(2,'0')).join(':');
    }else if((trades&&column===4)||(period&&column===0&&v>20000)||analysisDate||settingsDate||(section==='交易分析'&&column>0&&(row===37||row===40))){
      const date=new Date(Date.UTC(1899,11,30)+Math.round(v*86400)*1000).toISOString();
      rendered=date.slice(0,10).replaceAll('-','/')+(settingsDate||(analysisDate&&row>=65)?' '+date.slice(11,19):'');
    }else{
      const count=(trades&&[0,1,7].includes(column))||(period&&[0,5].includes(column))
        ||(section==='策略分析'&&[24,33].includes(row))||(section==='交易分析'&&row>=4&&row<=7)
        ||(section==='設定'&&label==='最長連虧月數');
      const isRiskRatio=section==='策略分析'&&column===1&&row>=39&&row<=45;
      const formatted=(isRiskRatio?riskRatio:count?integer:amount).format(Math.abs(v));
      negative=v<0&&Number(formatted.replaceAll(',',''))!==0;
      rendered=negative?'('+formatted+')':formatted;
    }
  }
  return '<'+tag+(header?' scope="col"':'')+(rowspan?' rowspan="2"':'')+
    (negative?' class="negative num"':typeof v==='number'?' class="num"':'')+'>'+escape(rendered)+'</'+tag+'>';
}
function supplemental(p){
  if(!p)return [];
  const totals = p.market_coverage?.last_check?.source_totals;
  const check = p.market_coverage?.last_check;
  const c = p.market_coverage;
  const checks = {checked:'已檢查',stale:'檢查已過期',partial_range:'檢查涵蓋部分期間',out_of_range:'檢查與回測期間不重疊',source_mismatch:'檢查對應不同來源或版本',not_checked:'尚無檢查紀錄',not_recorded:'舊回測未保存檢查紀錄',unavailable:'無法讀取檢查紀錄'};
  return [
    ['補充損益（MCP）'],['扣費前損益',p.gross_pnl],['手續費',p.commission],
    ['扣費後淨損益',p.net_pnl],['日均交易筆數',p.average_daily_trades],['勝率 (%)',p.win_rate_pct],
    ['賺賠比',p.payoff_ratio],['單日最大回撤',p.maximum_daily_drawdown],
    ['單日最大回撤上界',p.maximum_daily_drawdown_upper_bound],['單日最大回撤下界',p.maximum_daily_drawdown_lower_bound],
    ['累積最大回撤',p.maximum_cumulative_drawdown],['最長連虧月數',p.longest_losing_months],
    ['統計日期基準',p.trading_day_basis],['有行情的統計日數',p.trading_days],['最大單日回撤日期',p.maximum_daily_drawdown_date],
    ['補充月損益（依上述統計日期）','平倉筆數','扣費後淨損益'],
    ...Object.entries(p.monthly ?? {}).map(([key,n]) => [key,n.trades,n.net_pnl]),
    ['補充年損益（依上述統計日期）','平倉筆數','扣費後淨損益'],
    ...Object.entries(p.yearly ?? {}).map(([key,n]) => [key,n.trades,n.net_pnl]),
    ...(c ? [
      ['行情完整性（回測時快照）',checks[c.status] ?? c.status],
      ['行情來源',c.source,c.kind,c.dataset_id],['行情資料庫',c.database],
      ['實際 K 棒起訖（含暖機）',c.first_bar,c.last_bar],
      ...(check ? [['最後檢查',check.checked_at,check.from,check.to,check.stale ? '已過期' : '當時有效']] : []),
      ...(totals ? [['檢查全期間缺漏',totals.missing,totals.missing_unit],
        ...c.overlapping_gap_examples.map(g => ['與回測重疊的缺口',g.from,g.to]),
        ...(c.gap_examples_truncated ? [['缺口明細為節錄']] : [])] : []),
    ] : []),
  ];
}
function writeRow(name, number, row, settingsDates, header=false, tradeRowIndex=null){
  const cells=row.map((value,column)=>{
    const merged=tradeRowIndex!==null&&tradeMerge.has(column);
    if(merged&&tradeRowIndex%2===1)return '';
    return htmlCell(name,number,column,value,settingsDates,header,merged,row[0]);
  }).join('');
  return '<tr data-row="'+number+'">'+cells+'</tr>';
}

function writeTable({name,first,rows}, charts, settingsDates){
  let table=[];
  const content=[];
  const flush=()=>{
    if(table.length){content.push('<table><tbody>'+table.join('')+'</tbody></table>');table=[];}
  };
  rows.forEach((row,index)=>{
    const number=first+index;
    const chart=charts.get(number);
    if(chart){
      flush();
      content.push('<figure><h3>'+escape(chart.title)+'</h3><img loading="lazy" width="800" height="800" src="'+escape(chart.file)+'" alt="'+escape(chart.title)+'"></figure>');
      return;
    }
    if(!nonempty(row)){
      if(table.length&&index+1<rows.length&&nonempty(rows[index+1]))table.push('<tr class="spacer"><td colspan="'+row.length+'"></td></tr>');
      return;
    }
    const header=(number===3&&name!=='設定')||(name==='週期性分析'&&(row[0]==='期間'||row[0]==='日期'));
    table.push(writeRow(name,number,row,settingsDates,header));
  });
  flush();
  return content.join('');
}

function setupTradePager(pager){
  const form=pager.querySelector('form');
  const input=form.elements.page;
  const previous=form.querySelector('[data-previous]');
  const next=form.querySelector('[data-next]');
  const status=pager.querySelector('[role="status"]');
  const body=pager.querySelector('tbody');
  const firstRows=body.innerHTML;
  const last=Number(input.max);
  let current=1;

  function show(page,rows){
    body.innerHTML=rows;
    current=page;
    input.value=page;
    previous.disabled=page===1;
    next.disabled=page===last;
    status.textContent='第 '+page+' 頁，共 '+last+' 頁';
  }
  function load(page){
    if(!Number.isInteger(page)||page<1||page>last||page===current||pager.hasAttribute('aria-busy'))return;
    if(page===1){show(page,firstRows);return;}
    pager.setAttribute('aria-busy','true');
    const script=document.createElement('script');
    script.src=pager.dataset.directory+'/page-'+page+'.js';
    function finish(){script.remove();pager.removeAttribute('aria-busy');}
    function failed(){
      finish();
      input.value=current;
      status.textContent='第 '+page+' 頁載入失敗，目前顯示第 '+current+' 頁。';
    }
    script.onload=()=>{
      if(typeof script.tradeRows!=='string'){failed();return;}
      show(page,script.tradeRows);
      finish();
    };
    script.onerror=failed;
    document.head.append(script);
  }
  form.addEventListener('submit',event=>{event.preventDefault();load(input.valueAsNumber);});
  previous.addEventListener('click',()=>load(current-1));
  next.addEventListener('click',()=>load(current+1));
  show(1,firstRows);
  form.hidden=false;
}

async function writeTradePages(output,{name,first,rows},settingsDates){
  const header=writeRow(name,first+2,rows[2],settingsDates,true);
  const start=rows[3]?.length===0?4:3;
  const pageSize=200; // 100 complete entry/exit pairs.
  const total=Math.ceil((rows.length-start)/pageSize);
  const directoryName=path.basename(output,'.html')+'-trades';
  const directory=path.join(path.dirname(output),directoryName);
  const renderPage=page=>{
    const begin=start+(page-1)*pageSize;
    return rows.slice(begin,begin+pageSize).map((row,index)=>
      writeRow(name,first+begin+index,row,settingsDates,false,index)).join('');
  };
  if(total>1){
    await fs.mkdir(directory);
    for(let page=2;page<=total;page++){
      const code='document.currentScript.tradeRows='+JSON.stringify(renderPage(page))+';';
      await fs.writeFile(path.join(directory,'page-'+page+'.js'),code,{flag:'wx'});
    }
  }
  const firstPage=total?renderPage(1):'';
  const controls=total>1?'<form class="trade-controls" hidden><button type="button" data-previous>上一頁</button> '+
    '<label>頁碼 <input name="page" type="number" min="1" max="'+total+'" value="1" required></label> '+
    '<button type="submit">前往</button> <button type="button" data-next>下一頁</button> '+
    '<span role="status"></span></form><noscript><p>請啟用 JavaScript 以瀏覽後續頁面。</p></noscript>':'';
  const pager='<div id="trade-pager" data-directory="'+escape(encodeURIComponent(directoryName))+'">'+controls+
    '<table><thead>'+header+'</thead><tbody>'+firstPage+'</tbody></table></div>';
  return pager+(total>1?'<script>('+setupTradePager.toString()+')(document.getElementById("trade-pager"));</script>':'');
}

export async function writeTransportReport({output,format,tables,performance,charts,style,r,settingsDates}){
  tables.at(-1).rows.push([], ...supplemental(performance));
  const chartMap=new Map();
  for(const chart of charts){
    if(!chartMap.has(chart.section))chartMap.set(chart.section,new Map());
    chartMap.get(chart.section).set(chart.row,chart);
  }
  if(format==='csv'){
    const rows=[['區段','列號',...columns]];
    for(const table of tables){
      const {name,title,first}=table;
      table.rows.forEach((row,i)=>{if(nonempty(row))rows.push([title,first+i,...row.map(normalize)]);});
      if(style==='simple'&&chartMap.get(name)?.has(first)){
        const combined=first===433;
        let equity=r.config.initial_capital, peak=equity;
        rows.push([title,'','累計平倉筆數','出場時間','平倉權益',...(combined?['平倉權益拉回']:[])]);
        rows.push([title,'',0,'初始',equity,...(combined?[0]:[])]);
        r.trades.forEach((trade,index)=>{
          equity+=trade.net;
          peak=Math.max(peak,equity);
          rows.push([title,'',index+1,trade.exit_datetime,equity,...(combined?[equity-peak]:[])]);
        });
      }
    }
    const csv=rows.map(row=>[...row,...Array(Math.max(0,18-row.length)).fill('')].map(csvCell).join(',')).join('\r\n');
    await fs.writeFile(output,'\uFEFF'+csv+'\r\n',{flag:'wx'});
    return;
  }
  const contents=(await Promise.all(tables.map(async(table,i)=>{
    const range=new Map([...(chartMap.get(table.name)??[])].filter(([row])=>row>=table.first&&row<table.first+table.rows.length));
    const body=table.name==='交易明細' ? await writeTradePages(output,table,settingsDates) : writeTable(table,range,settingsDates);
    return '<section id="section-'+i+'"><h2>'+escape(table.title)+'</h2>'+
      body+'</section>';
  }))).join('');
  const title=style==='simple'?'簡式策略績效報告':'策略績效報告';
  await fs.writeFile(output,'<!doctype html><html lang="zh-Hant"><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>'+title+'</title>'+
    '<style>body{font:16px "PMingLiU","新細明體",serif;margin:2rem;color:#171717}section{margin:2rem 0;overflow:auto}table{border-collapse:collapse}td,th{padding:.23rem .6rem;border:1px solid #d8d8d8;white-space:nowrap}th{text-align:left}td:first-child{font-weight:bold}.num{text-align:right;font-variant-numeric:tabular-nums}.negative{color:#b42318}.spacer td{height:.8rem}img{max-width:100%;height:auto}figure{margin:1rem 0}nav a{display:inline-block;margin:.2rem .75rem .2rem 0}.trade-controls{margin:0 0 .6rem}.trade-controls input{width:5em}.trade-controls [role=status]{margin-left:.5em}#trade-pager[aria-busy] .trade-controls{cursor:progress}</style>'+
    '<h1>'+title+'</h1><nav>'+tables.map(({title},i)=>'<a href="#section-'+i+'">'+escape(title)+'</a>').join('')+'</nav>'+contents+'</html>',{flag:'wx'});
}

