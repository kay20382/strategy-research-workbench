param([switch]$Excel)
$ErrorActionPreference='Stop'
Push-Location $PSScriptRoot
try {
    $out=Join-Path $PSScriptRoot ('outputs/godot-workbench-test-'+[guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Path $out | Out-Null
    $env:APPDATA=Join-Path $PSScriptRoot '.tools/appdata'
    $options=@('--headless','--path','godot-workbench','res://main.tscn','--quit-after','36000','--',"--smoke-directory=$out")
    if($Excel){$options+='--smoke-excel'}
    $result=& .tools/godot/Godot_v4.7.2-stable_win64_console.exe @options 2>&1
    $code=$LASTEXITCODE
    [IO.File]::WriteAllText((Join-Path $out 'godot.log'),($result -join "`n"),(New-Object Text.UTF8Encoding($false)))
    if($code -ne 0 -or !($result -match 'GODOT_WORKBENCH_SMOKE_OK')) {throw "Godot工作台測試失敗：$out/godot.log"}
    $proof=Get-Content (Join-Path $out 'verification.json') -Raw|ConvertFrom-Json
    if($Excel -and !$proof.excel_checked){throw 'Excel未經驗證'}
    [pscustomobject]@{passed=$proof.passed;excelChecked=$proof.excel_checked;firstTrades=$proof.receipts[0].summary.trades;changedTrades=$proof.receipts[1].summary.trades;evidence=$out}|ConvertTo-Json
    Write-Output $out
} finally {Pop-Location}
