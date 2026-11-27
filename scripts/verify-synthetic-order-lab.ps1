param(
    [Parameter(Mandatory=$true)][string]$RunDirectory,
    [Parameter(Mandatory=$true)][string]$OutputPath
)
$ErrorActionPreference = 'Stop'
if (Test-Path -LiteralPath $OutputPath) { throw '驗證輸出已存在，拒絕覆寫。' }
$expected = @(Get-Content data/synthetic-order-lab/manual-expectations.json -Raw | ConvertFrom-Json)
$report = Get-Content (Join-Path $RunDirectory 'engine/report.json') -Raw | ConvertFrom-Json
if (@($report.trades).Count -ne 12 -or $report.bar_count -ne 171) { throw '棒數／交易數不符。' }
$checks = foreach ($index in 0..11) {
    $e=$expected[$index]; $t=$report.trades[$index]
    foreach ($pair in @(@('side','side'),@('quantity','quantity'),@('entry_price','entry'),@('exit_price','exit'),@('gross','gross'),@('costs','commission'),@('net','net'))) {
        if ($t.($pair[0]) -ne $e.($pair[1])) { throw "案例$($index+1)欄位$($pair[0])不符人工期望。" }
    }
    [pscustomobject]@{ case=$index+1; entry=$t.entry_price;exit=$t.exit_price;net=$t.net;entry_time=$t.entry_datetime;exit_time=$t.exit_datetime;entry_label=$t.entry_label;exit_label=$t.exit_label;passed=$true }
}
if ($report.summary.net_closed -ne -8 -or $report.summary.commissions -ne 28) { throw '摘要對帳失敗。' }
if ($report.pending_at_end -ne 1 -or $report.final_position -ne 0) { throw '尾棒狀態不符。' }
if (@($report.fills | Where-Object label -eq 'S12_FINAL_PENDING').Count) { throw '次棒不存在卻成交。' }
if (@($report.fills | Where-Object label -eq 'S09_EXPIRE').Count) { throw '已過期遠價單卻成交。' }
if (@($report.events | Where-Object { $_.label -eq 'S09_EXPIRE' -and $_.status -eq 'expired' }).Count -ne 1) { throw '未找到遠價單到期。' }
$last=$report.trades[11]
if ($last.exit_label -ne 'S12_FINAL_CLOSE' -or $last.exit_datetime -ne '2026-10-26T11:51:00') { throw '未在最後一棒收盤平倉。' }
$result=[pscustomobject]@{ run=$RunDirectory; source_sha256=$report.strategy_sha256;checks=$checks;summary=$report.summary;pending_at_end=$report.pending_at_end;final_position=$report.final_position;limitations='S05/S06期望採近端極值且等距先低假設；待MC對照，未據此宣稱原生棒內路徑已確認。' }
$result | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $OutputPath -Encoding utf8
$checks | Format-Table case,entry,exit,net,exit_label,passed
