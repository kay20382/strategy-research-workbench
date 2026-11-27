// 驗證敘述必須綁定本次引擎產物；不能沿用舊任務的績效結論。
export function mcReportValidationNote(report,sha256,comparison){
 if(!comparison)return '未載入綁定本次引擎產物雜湊的 MC 比較紀錄；本報告不得據舊版驗證敘述宣稱與 MC 等價。';
 if(comparison.engine_report_sha256!==sha256)throw new Error('MC 比較紀錄不屬於本次引擎產物，拒絕沿用驗證結論');
 if(comparison.schema==='mc-trade-comparison-v1')return `本次引擎 ${comparison.engine_trades} 筆已平倉，MC 參考 ${comparison.mc_trades} 筆；逐筆全部抽取欄位${comparison.exact?'相同':'仍有差異'}。此比較未完成報表期間、完整指標及版面等價驗證，不能據此宣稱完整報告相同。`;
 const source=comparison.reference_is_original?'原始 MC 報告':'MC 原生執行還原策略的參考報告';
 const notes=[comparison.closed_numeric_and_time_equal
  ?`相同截止範圍內 ${report.trades.length} 筆已平倉交易的數值與時間符合${source}。`
  :`本次已平倉交易數值或時間仍與${source}不同，未通過逐筆等價。`];
 if(!comparison.closed_signal_labels_equal)notes.push('訊號顯示文字仍有差異。');
 if(!comparison.full_summary_comparable)notes.push('報告截止時間不同，不能直接宣稱整份摘要與未平倉評價相同。');
 else if(comparison.full_summary_differences?.length)notes.push(`同截止摘要仍有 ${comparison.full_summary_differences.length} 項差異。`);
 else notes.push('同截止已比較的摘要指標相同。');
 if(comparison.open_position?.status==='different')notes.push('未平倉部位尚有差異。');
 if(!comparison.all_report_metrics_validated||!comparison.all_report_layout_validated)notes.push('完整報表指標及版面尚未全面驗證。');
 return notes.join('');
}
