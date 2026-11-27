param([switch]$VersionOnly)
$ErrorActionPreference='Stop'
Push-Location $PSScriptRoot
try {
    $out=Join-Path $PSScriptRoot ('outputs/studio-e2e-'+[guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Path $out | Out-Null
    $db=Join-Path $out 'market.sqlite'
    Copy-Item data/synthetic-order-lab/market.sqlite $db
    $instrument=@{
        symbol='REA_SYNTH'; name='人工行情'; timezone='Asia/Taipei'; price_scale=1
        minimum_move=1; point_value=1; confirmed=$true
        trading_weekdays=@(1,2,3,4,5); closed_dates=@(); auto_fill=$false
        sessions=@(@{name='全天'; open='00:00:00'; close='00:00:00'; trading_day_shift=0; weekdays=@(1,2,3,4,5)})
    }
    $file=Join-Path $out 'instrument.json'
    $encoding=[Text.UTF8Encoding]::new($false)
    [IO.File]::WriteAllText($file,($instrument | ConvertTo-Json -Depth 6),$encoding)
    & rust/target/release/quote_manager.exe save $db $file
    if($LASTEXITCODE -ne 0){throw '無法設定測試商品'}
    $env:APPDATA=Join-Path $out 'appdata'
    $env:STRATEGY_TRADE_JOURNAL=Join-Path $out 'trade.sqlite'
    $env:STRATEGY_VERSION_GIT_DIR=Join-Path $out 'versions.git'
    $env:STRATEGY_LIVE_MARKET_DB=Join-Path $out 'empty-live.sqlite'
    $mode = if($VersionOnly){'--test-version-control'}else{'--open-quote'}
    $result=& .tools/godot/Godot_v4.7.2-stable_win64_console.exe --headless --path godot-workbench --quit-after 20000 --script res://studio_test.gd -- $mode "--database=$db" 2>&1
    $code=$LASTEXITCODE
    [IO.File]::WriteAllText((Join-Path $out 'godot.log'),($result -join "`n"),$encoding)
    $required = @('STRATEGY_VERSION_OK','STRATEGY_VERSION_SHARED')
    if(!$VersionOnly){$required += @('STUDIO_OK','TRADE_REPLAY_OK','STRATEGY_EXECUTION_OK','ROBUSTNESS_OK','STRATEGY_LOGIC_OK','FOREIGN_ORDER_OK','FOREIGN_STRATEGY_REPLAY_OK')}
    $missing = @($required | Where-Object { !($result -match ('^' + $_ + ':')) })
    if($code -ne 0 -or $missing.Count -gt 0 -or ($result -match 'SCRIPT ERROR|^ERROR:')) {
        throw "整合測試失敗，請見 $out/godot.log"
    }
    $result | Where-Object {$_ -match '^(STUDIO_OK|TRADE_REPLAY_OK|STRATEGY_EXECUTION_OK|ROBUSTNESS_OK|STRATEGY_VERSION_OK|STRATEGY_LOGIC_OK|FOREIGN_ORDER_OK|FOREIGN_STRATEGY_REPLAY_OK):'}
    $shared = [string]($result | Where-Object {$_ -match '^STRATEGY_VERSION_SHARED: '})
    $version = $shared -replace '^STRATEGY_VERSION_SHARED: ',''
    if($version -notmatch '^[0-9a-f]{40}$'){throw '桌面未提交可供 MCP 查詢的版本'}
    $jsonRpc = @(
        @{jsonrpc='2.0';id=1;method='initialize';params=@{protocolVersion='2025-11-25';capabilities=@{};clientInfo=@{name='studio-test';version='1'}}},
        @{jsonrpc='2.0';method='notifications/initialized'},
        @{jsonrpc='2.0';id=2;method='tools/call';params=@{name='strategy_version';arguments=@{strategy_id='REA_SyntheticOrderLab';operation='list'}}},
        @{jsonrpc='2.0';id=3;method='tools/call';params=@{name='strategy_version';arguments=@{strategy_id='REA_SyntheticOrderLab';operation='draft';version=$version}}}
    ) | ForEach-Object {$_ | ConvertTo-Json -Depth 7 -Compress}
    $server = (Get-Content strategy-mcp.json -Raw | ConvertFrom-Json).mcpServers.strategy_studio.command
    $reply = $jsonRpc | & $server --database $db | ForEach-Object {$_ | ConvertFrom-Json}
    $history = ($reply | Where-Object id -eq 2).result.structuredContent
    $draft = ($reply | Where-Object id -eq 3).result.structuredContent
    if($LASTEXITCODE -ne 0 -or $history.versions.Count -ne 1 -or $history.versions[0].version -ne $version -or !$draft.dirty -or !$draft.source.Contains('S01')){throw 'MCP 未能讀取並還原桌面提交的版本'}
    Write-Output "STRATEGY_VERSION_SHARED_OK: $version"
    if($VersionOnly){Write-Output $out; return}
    $env:STRATEGY_API_PROFILE_DB=Join-Path $out 'chat-api.sqlite'
    $env:STRATEGY_HISTORICAL_MARKET_DB=$db
    $env:STRATEGY_LIVE_MARKET_DB=Join-Path $out 'empty-live.sqlite'
    $chat=& .tools/godot/Godot_v4.7.2-stable_win64_console.exe --headless --path godot-workbench --quit-after 20000 --script res://studio_test.gd -- --test-chat-home "--database=$db" 2>&1
    $code=$LASTEXITCODE
    [IO.File]::WriteAllText((Join-Path $out 'chat.log'),($chat -join "`n"),$encoding)
    if($code -ne 0 -or !($chat -match '^CHAT_HOME_OK:') -or !($chat -match '^TELEGRAM_OK:') -or !($chat -match '^CHART_STRATEGY_OK$') -or ($chat -match 'SCRIPT ERROR|^ERROR:')) {
        throw "對話首頁測試失敗，請見 $out/chat.log"
    }
    $settingsDbText=$encoding.GetString([IO.File]::ReadAllBytes($env:STRATEGY_API_PROFILE_DB))
    if($settingsDbText.Contains('fixture-openai-key-224c') -or $settingsDbText.Contains('fixture-Telegram-224c') -or
       $settingsDbText.Contains('fixture-gemini-key-224c') -or $settingsDbText.Contains('fixture-anthropic-key-224c') -or
       $settingsDbText.Contains('fixture-grok-key-224c') -or $settingsDbText.Contains('fixture-mixroute-key-224c')){
        throw '工作台設定資料庫含有憑證明文'
    }
    $chat | Where-Object {$_ -match '^(CHAT_HOME_OK|TELEGRAM_OK):|^CHART_STRATEGY_OK$'}
    Write-Output $out
} finally {Pop-Location}
