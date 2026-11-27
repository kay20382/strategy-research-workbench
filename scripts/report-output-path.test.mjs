import test from 'node:test';
import assert from 'node:assert/strict';
import {reportOutputPath} from './report-output-path.mjs';
const prefs={directory:'C:\\WorkSpace\\Codex\\REA',filename_pattern:'{symbol} {strategy}回測報告.xlsx'};
const r={provenance:{symbol:'TXF1'},config:{strategy_name:'REA_KGI_07_YAO1_Fix1'}};
test('預設位置及使用者最後指定的檔名格式',()=>{
 assert.equal(reportOutputPath(r,null,prefs),'C:\\WorkSpace\\Codex\\REA\\TXF1 REA_KGI_07_YAO1_Fix1回測報告.xlsx');
 assert.equal(reportOutputPath(r,'-',prefs),reportOutputPath(r,null,prefs));
});
test('明示輸出路徑維持既有介面',()=>assert.equal(reportOutputPath(r,'outputs/test.xlsx',prefs),'outputs/test.xlsx'));
test('非法檔名字元或缺少身分不自行改名',()=>{
 assert.throws(()=>reportOutputPath({...r,provenance:{symbol:'TX:202611'}},null,prefs));
 assert.throws(()=>reportOutputPath({...r,config:{}},null,prefs));
});
