param([switch]$Build,[switch]$Legacy)
$ErrorActionPreference = 'Stop'
if(!$Legacy) {
    & (Join-Path $PSScriptRoot 'pl-workbench-godot.ps1') -Build:$Build
    return
}
Push-Location $PSScriptRoot
try {
    if ($Build -or -not (Test-Path -LiteralPath 'rust\target\debug\backtest.exe') -or -not (Test-Path -LiteralPath 'rust\target\debug\pl_strategy_manager.exe')) {
        & .\build.ps1
    }
    Start-Process -FilePath (Join-Path $PSScriptRoot 'rust\target\debug\pl_strategy_manager.exe') -WorkingDirectory $PSScriptRoot -WindowStyle Normal
} finally { Pop-Location }
