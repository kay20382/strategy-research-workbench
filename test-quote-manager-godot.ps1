$ErrorActionPreference='Stop'
Push-Location $PSScriptRoot
try {
    $out=Join-Path $PSScriptRoot ('outputs/godot-ui-test-'+[guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Path $out | Out-Null
    $utf8=New-Object System.Text.UTF8Encoding($false)
    [IO.File]::WriteAllText((Join-Path $out 'ticks.csv'),"Date,Time,Price,Volume`n2026/10/26,09:00:00,100,2`n2026/10/26,09:00:00,100,2`n2026/10/26,09:01:02,110,1`n2026/10/26,09:03:00,999,7`n",$utf8)
    [IO.File]::WriteAllText((Join-Path $out 'bars.csv'),"DateTime,Open,High,Low,Close,Volume`n20261026 09:03,111,113,110,112,1`n",$utf8)
    $env:APPDATA=Join-Path $PSScriptRoot '.tools/appdata'
    $result=& .tools/godot/Godot_v4.7.2-stable_win64_console.exe --headless --path godot-workbench res://quote/main.tscn --quit-after 12000 -- --smoke-test "--database=$out/market.sqlite" 2>&1
    $code=$LASTEXITCODE
    [IO.File]::WriteAllText((Join-Path $out 'godot.log'),($result -join "`n"),$utf8)
    if($code -ne 0 -or !($result -match 'GODOT_UI_SMOKE_OK')) {throw "Godot端到端測試失敗，請見 $out/godot.log"}
    $restore=& .tools/godot/Godot_v4.7.2-stable_win64_console.exe --headless --path godot-workbench res://quote/main.tscn --quit-after 12000 -- --smoke-restore "--database=$out/market.sqlite" 2>&1
    $restoreCode=$LASTEXITCODE
    [IO.File]::WriteAllText((Join-Path $out 'restore.log'),($restore -join "`n"),$utf8)
    if($restoreCode -ne 0 -or !($restore -match 'GODOT_COVERAGE_RESTORE_OK')) {throw "重開恢復最後紀錄測試失敗，請見 $out/restore.log"}
    $proof=@{passed=$true;evidence=$out;checks=@('商品儲存與更新後重讀','錯選Tick可改選1分K','Tick匯入','自動補建','三種來源','缺漏查詢','重複補建','Tick匯出','匯入後保留舊結果並提示過期','切換商品還原紀錄','獨立程序重開還原紀錄與過期提示')}
    [IO.File]::WriteAllText((Join-Path $out 'verification.json'),($proof|ConvertTo-Json -Depth 5),$utf8)
    $proof|ConvertTo-Json
} finally {Pop-Location}
