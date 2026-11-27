import test from 'node:test';
import assert from 'node:assert/strict';
import {mcReportValidationNote} from './mc-report-validation-note.mjs';
const report={trades:[{},{}]};
const receipt={engine_report_sha256:'本次',reference_is_original:true,closed_numeric_and_time_equal:true,closed_signal_labels_equal:true,full_summary_comparable:true,full_summary_differences:[],open_position:{status:'matched'},all_report_metrics_validated:false,all_report_layout_validated:false};

test('逐筆比較收據不能自行推論報告截止時間不同',()=>{
 const note=mcReportValidationNote(report,'本次',{schema:'mc-trade-comparison-v1',engine_report_sha256:'本次',engine_trades:607,mc_trades:589,exact:false});
 assert.match(note,/607.*589/);assert.match(note,/仍有差異/);assert.doesNotMatch(note,/截止時間不同/);
});
test('rejects stale comparison evidence rather than reusing a prior success',()=>{
 assert.throws(()=>mcReportValidationNote(report,'新版',receipt),/不屬於本次/);
 assert.match(mcReportValidationNote(report,'本次',null),/不得據舊版/);
});
test('keeps original and restored reference evidence and time scopes distinct',()=>{
 assert.match(mcReportValidationNote(report,'本次',receipt),/2 筆.*符合原始 MC/);
 const partial=mcReportValidationNote(report,'本次',{...receipt,reference_is_original:false,closed_signal_labels_equal:false,full_summary_comparable:false});
 assert.match(partial,/還原策略/);assert.match(partial,/截止時間不同/);assert.match(partial,/訊號顯示文字/);assert.match(partial,/尚未全面驗證/);
});
