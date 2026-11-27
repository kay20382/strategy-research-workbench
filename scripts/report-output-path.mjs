import path from 'node:path';

export function reportOutputPath(report,requested,preferences){
 if(requested&&requested!=='-')return requested;
 const symbol=report.provenance?.symbol;
 const strategy=report.config?.strategy_name;
 for(const [name,value]of [['商品',symbol],['策略名稱',strategy]]){
  if(typeof value!=='string'||!value.trim()||/[<>:"/\\|?*\u0000-\u001f]/.test(value)||/[. ]$/.test(value))throw new Error(`${name}無法作為 Windows 檔名：${value}`);
 }
 const filename=preferences.filename_pattern.replace('{symbol}',symbol).replace('{strategy}',strategy);
 return path.win32.join(preferences.directory,filename);
}
