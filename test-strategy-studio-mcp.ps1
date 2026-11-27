$ErrorActionPreference = 'Stop'
$root = $PSScriptRoot
$clientConfig = Get-Content (Join-Path $root 'strategy-mcp.json') -Raw | ConvertFrom-Json
$server = $clientConfig.mcpServers.strategy_studio.command
$fixture = Join-Path $root 'data/synthetic-order-lab/market.sqlite'
$out = Join-Path $root ('outputs/studio-mcp-test-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $out | Out-Null
$proof = [ordered]@{}

function Assert-Test($condition, [string]$reason) {
    if (!$condition) { throw $reason }
}

function Send-Message($message) {
    $bytes = [Text.Encoding]::UTF8.GetBytes(($message | ConvertTo-Json -Depth 16 -Compress) + "`n")
    $process.StandardInput.BaseStream.Write($bytes, 0, $bytes.Length)
    $process.StandardInput.BaseStream.Flush()
}

function Receive-Message([string]$requestLabel) {
    $line = $process.StandardOutput.ReadLineAsync()
    if (!$line.Wait(45000) -or !$line.Result) { throw "MCP 回覆逾時或連線已結束（$requestLabel）" }
    return ($line.Result | ConvertFrom-Json)
}

function Invoke-Rpc([int]$id, [string]$method, $parameters) {
    Send-Message @{jsonrpc='2.0'; id=$id; method=$method; params=$parameters}
    $reply = Receive-Message "$id $method $($parameters.name)"
    Assert-Test ($reply.jsonrpc -eq '2.0' -and $reply.id -eq $id) 'JSON-RPC 回覆格式或編號錯誤'
    return $reply
}

function Invoke-Tool([int]$id, [string]$name, $arguments, [switch]$ExpectError) {
    $reply = Invoke-Rpc $id 'tools/call' @{name=$name; arguments=$arguments}
    Assert-Test ($null -ne $reply.result) "工具 $name 缺少回覆"
    Assert-Test ([bool]$reply.result.isError -eq $ExpectError.IsPresent) "$name $($reply.result.content[0].text)"
    return $reply.result.structuredContent
}

$start = [Diagnostics.ProcessStartInfo]::new($server, ('--database "' + $fixture + '"'))
$start.UseShellExecute = $false
$start.CreateNoWindow = $true
$start.RedirectStandardInput = $true
$start.RedirectStandardOutput = $true
$start.RedirectStandardError = $true
$start.StandardOutputEncoding = [Text.Encoding]::UTF8
$start.StandardErrorEncoding = [Text.Encoding]::UTF8
$start.EnvironmentVariables['REA_MCP_LOOP_DIRECTORY'] = Join-Path $out 'loops'
$start.EnvironmentVariables['STRATEGY_API_PROFILE_DB'] = Join-Path $out 'api.sqlite'
$start.EnvironmentVariables['STRATEGY_VERSION_GIT_DIR'] = Join-Path $out 'versions.git'
$process = [Diagnostics.Process]::Start($start)
$diagnostics = $process.StandardError.ReadToEndAsync()
try {
    $before = Invoke-Rpc 1 'tools/list' @{}
    Assert-Test ($before.error.code -eq -32002) '初始化前接受工具呼叫'
    $init = Invoke-Rpc 2 'initialize' @{protocolVersion='2025-11-25'; capabilities=@{}; clientInfo=@{name='rea-test'; version='1'}}
    Assert-Test ($init.result.protocolVersion -eq '2025-11-25') '協定交握失敗'
    Send-Message @{jsonrpc='2.0'; method='notifications/initialized'}
    $list = Invoke-Rpc 3 'tools/list' @{}
    $toolNames = $list.result.tools.name
    Assert-Test ($toolNames.Count -ge 24) '工具清單不完整'
    foreach ($jobTool in 'job_start', 'job_status', 'job_cancel') {
        Assert-Test ($toolNames -contains $jobTool) "工具清單缺少 $jobTool"
    }
    $apiSchema = Invoke-Tool 35 'api_profile_schema' @{provider='kgi_quotecom'; purpose='quote'}
    Assert-Test ($apiSchema.fields.Count -eq 6 -and $apiSchema.adapter_available -and !$apiSchema.supports_custom_fields -and $apiSchema.fields.key -contains 'account') '凱基欄位定義不符'
    $customSchema = Invoke-Tool 36 'api_profile_schema' @{provider='fixture_broker'; purpose='trade'}
    Assert-Test ($customSchema.supports_custom_fields -and !$customSchema.adapter_available) '自訂券商欄位定義不符'
    $emptyProfiles = Invoke-Tool 30 'api_profile_list' @{}
    Assert-Test ($emptyProfiles.profiles.Count -eq 0) '隔離 API 設定庫應為空'
    $apiProfile = @{id='gateway-test'; provider='kgi_quotecom'; purpose='quote'; host='localhost'; port=5432; source_id='TEST'; ca_path=(Join-Path $out 'fixture.pfx')}
    $testSecret = 'fixture-secret-' + [guid]::NewGuid().ToString('N')
    $apiSaved = Invoke-Tool 31 'api_profile_save' @{profile=$apiProfile; credentials=@{account=$testSecret; password=$testSecret; token=$testSecret; ca_password=$testSecret}}
    Assert-Test ($apiSaved.password_set -and $apiSaved.token_set -and $apiSaved.ca_password_set -and $apiSaved.credentials_readable) '無法儲存 API 憑證'
    Assert-Test (!(($apiSaved | ConvertTo-Json -Depth 5).Contains($testSecret))) '儲存回覆包含憑證明文'
    $apiSaved = Invoke-Tool 32 'api_profile_save' @{profile=$apiProfile; credentials=@{ca_password=''}}
    Assert-Test ($apiSaved.password_set -and !$apiSaved.ca_password_set) '空字串應清除憑證，省略欄位應保留'
    $apiListed = Invoke-Tool 33 'api_profile_list' @{}
    Assert-Test ($apiListed.profiles[0].profile.id -eq 'gateway-test' -and $apiListed.profiles[0].account_set) '無法讀回 API 設定'
    $dbText = [Text.Encoding]::UTF8.GetString([IO.File]::ReadAllBytes((Join-Path $out 'api.sqlite')))
    Assert-Test (!$dbText.Contains($testSecret)) '設定資料庫包含憑證明文'
    $previousApiDb = $env:STRATEGY_API_PROFILE_DB
    $previousAppData = $env:APPDATA
    try {
        $env:STRATEGY_API_PROFILE_DB = $start.EnvironmentVariables['STRATEGY_API_PROFILE_DB']
        $env:APPDATA = Join-Path $out 'appdata'
        $godot = Join-Path $root '.tools/godot/Godot_v4.7.2-stable_win64_console.exe'
        $ui = & $godot --headless --path (Join-Path $root 'godot-workbench') --quit-after 2000 --script res://studio_test.gd -- --test-api-profiles 2>&1
        [IO.File]::WriteAllText((Join-Path $out 'api-ui.log'),($ui -join "`n"),[Text.Encoding]::UTF8)
        Assert-Test ($LASTEXITCODE -eq 0 -and ($ui -match '^API_PROFILES_UI_OK$') -and !($ui -match 'SCRIPT ERROR|^ERROR:')) 'API 設定畫面驗證失敗，請查看 api-ui.log'
        $apiListed = Invoke-Tool 34 'api_profile_list' @{}
        $tradeSchema = Invoke-Tool 40 'api_profile_schema' @{provider='kgi_tradecom'; purpose='trade'}
        Assert-Test ($tradeSchema.fields.Count -eq 7 -and $tradeSchema.adapter_available) 'TradeCom 欄位或轉接器狀態不符'
        $tradeProfile = $apiListed.profiles | Where-Object {$_.profile.id -eq 'tradecom-test'}
        Assert-Test ($tradeProfile.profile.options.broker_id -eq '0012' -and $tradeProfile.secret_keys -contains 'trade_account' -and $tradeProfile.secret_keys -contains 'sub_account') 'MCP 未讀到 TradeCom 設定'
        $tradeDb = [Text.Encoding]::UTF8.GetString([IO.File]::ReadAllBytes((Join-Path $out 'api.sqlite')))
        Assert-Test (!$tradeDb.Contains('0001234') -and !$tradeDb.Contains('0005678')) '交易帳號或子帳明文寫入資料庫'
        Assert-Test ($apiListed.profiles[0].profile.port -eq 5433 -and $apiListed.profiles[0].profile.ca_path.EndsWith('fixture.pfx') -and $apiListed.profiles[0].token_set) 'MCP 未讀到 UI 更新的設定'
        $customProfile = $apiListed.profiles | Where-Object {$_.profile.id -eq 'ui-custom'}
        Assert-Test ($customProfile.profile.options.environment -eq 'sandbox' -and $customProfile.secret_keys -contains 'api_key') 'MCP 未讀到 UI 的自訂參數'
        Assert-Test (!(($customProfile | ConvertTo-Json -Depth 8).Contains('ui-fixture-sensitive'))) '自訂敏感參數洩露'
        $retained = Invoke-Tool 37 'api_profile_save' @{profile=$customProfile.profile}
        Assert-Test ($retained.secret_keys -contains 'api_key') '省略自訂敏感欄位須沿用原值'
        $cleared = Invoke-Tool 38 'api_profile_save' @{profile=$customProfile.profile; credentials=@{secrets=@{api_key=''}}}
        Assert-Test ($cleared.secret_keys.Count -eq 0) '清除自訂敏感欄位失敗'
        $stub = 'if ($env:QUOTECOM_CONNECTION -ne "gateway-test" -or $env:QUOTECOM_SOURCE_ID -ne "TEST" -or $env:QUOTECOM_PORT -ne "5433" -or $env:QUOTECOM_HOST -ne "localhost" -or $env:QUOTECOM_ID -notlike "fixture-secret-*" -or $env:QUOTECOM_ID -ne $env:QUOTECOM_PASSWORD -or $env:QUOTECOM_TOKEN -ne $env:QUOTECOM_PASSWORD) { exit 9 }'
        & (Join-Path $root 'rust/target/debug/live_market.exe') (Join-Path $out 'live.sqlite') Asia/Taipei taifex --profile gateway-test --gateway powershell.exe -NoProfile -NonInteractive -EncodedCommand ([Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes($stub)))
        Assert-Test ($LASTEXITCODE -eq 0) '行情轉接子程序未收到 API 設定'
    } finally {
        $env:STRATEGY_API_PROFILE_DB = $previousApiDb
        $env:APPDATA = $previousAppData
    }
    $sources = Invoke-Tool 4 'market_sources' @{symbol='rea_synth'}
    Assert-Test ($sources.sources.Count -gt 0) '找不到人工行情來源'
    $records = Invoke-Tool 5 'market_query' @{symbol='REA_SYNTH'; kind='minute'; from='2026-10-26'; to='2026-10-26'; limit=3}
    Assert-Test ($records.records.Count -eq 3 -and $records.truncated) '行情筆數與截斷標記不符'
    $boundary = Invoke-Tool 6 'market_query' @{symbol='REA_SYNTH'; kind='minute'; from='2026-10-26T09:01:00'; to='2026-10-26T09:01:00'}
    Assert-Test ($boundary.records.Count -eq 1) '行情閉區間查詢不符'
    $catalog = Invoke-Tool 7 'strategy_list' @{}
    $fixtureEntry = $catalog.strategies | Where-Object id -eq 'REA_SyntheticOrderLab'
    Assert-Test ($null -ne $fixtureEntry) '策略庫缺少人工驗證策略'
    $strategy = Invoke-Tool 8 'strategy_read' @{strategy_id=$fixtureEntry.id}
    $annotated = "{ 繁體中文進場說明 }`n" + $strategy.source
    $sourceDelta = Invoke-Tool 69 'strategy_diff' @{before=@{strategy_id=$fixtureEntry.id};after=@{source=$annotated}}
    Assert-Test ($sourceDelta.added -eq 1 -and $sourceDelta.removed -eq 0 -and $sourceDelta.changes[0].new_line -eq 1 -and $sourceDelta.changes[0].text.Contains('繁體中文')) '程式碼比對遺漏中文註釋或行號'
    $view = Invoke-Tool 71 'strategy_diff' @{before=@{source='if x < 10 then y = 1;'};after=@{source='if x < 50 then y = 1;'}}
    $comparison = Get-Content -LiteralPath $view.report_path -Raw -Encoding UTF8
    Assert-Test ($comparison.Contains('修改前') -and $comparison.Contains('修改後') -and $comparison.Contains('&lt; <mark>1</mark>0') -and $comparison.Contains('&lt; <mark>5</mark>0')) 'HTML 未左右對照、醒目標示行內替換或正確跳脫原始碼'
    $separateEdits = Invoke-Tool 73 'strategy_diff' @{before=@{source='x=1; y=2;'};after=@{source='x=5; y=8;'}}
    $separateHtml = Get-Content -LiteralPath $separateEdits.report_path -Raw -Encoding UTF8
    Assert-Test ($separateHtml.Contains('x=<mark>1</mark>; y=<mark>2</mark>;') -and $separateHtml.Contains('x=<mark>5</mark>; y=<mark>8</mark>;') -and $separateHtml.Contains('id="overview"')) '同一行內分開的改動未各自標示'
    $encoded = Invoke-Tool 70 'strategy_export_pl' @{source=$annotated}
    if ('System.Text.CodePagesEncodingProvider' -as [type]) {
        [Text.Encoding]::RegisterProvider([Text.CodePagesEncodingProvider]::Instance)
    }
    $encodedText = [Text.Encoding]::GetEncoding(950).GetString([IO.File]::ReadAllBytes($encoded.path))
    Assert-Test ($encoded.encoding -eq 'windows-950' -and $encodedText.StartsWith("{ 繁體中文進場說明 }`r`n")) 'PowerLanguage 匯出未以 CP950 保存中文註釋'
    $review = Invoke-Tool 67 'strategy_requirements' @{strategy_id=$fixtureEntry.id;acceptance=@{pass_criteria='以原版為比較基準'}}
    Assert-Test ($init.result.instructions.Contains('使用者明確同意') -and $review.status -eq 'review_required' -and $review.checklist.Count -eq 5 -and @($review.checklist | Where-Object needs_definition).Count -eq 4 -and $review.mandatory.Contains('exit_bar')) 'MCP 未提供策略驗收確認流程'
    $questionnaire = Invoke-Tool 68 'strategy_requirements' @{request='幫我讓策略更穩'}
    Assert-Test ($questionnaire.questions.Count -eq 5 -and $review.questions.Count -eq 4 -and !($review.questions.key -contains 'pass_criteria')) 'MCP 未依已回答的條件更新問題'
    foreach ($question in $questionnaire.questions) {
        Assert-Test ($question.question -and $question.options[0].recommended -and $question.options[-1].label -eq '自行描述' -and $question.options[-1].free_text) 'MCP 缺少建議答案或自行描述選項'
    }
    $newStrategy = Invoke-Tool 72 'strategy_requirements' @{mode='new';request='幫我設計新策略';acceptance=@{product='TXF1'}}
    $newKeys = @('bar_period','market_session','strategy_goal','entry_rules','exit_rules','stop_loss','take_profit')
    foreach ($key in $newKeys) {
        $item = $newStrategy.questions | Where-Object key -eq $key
        Assert-Test ($item.question -and $item.options[-1].free_text) "新策略缺少必問項目 $key"
    }
    Assert-Test ($newStrategy.mode -eq 'new' -and !($newStrategy.questions.key -contains 'product') -and ($newStrategy.checklist | Where-Object key -eq 'product').criterion -eq 'TXF1') '新策略沒有沿用已確認商品'
    Assert-Test ($strategy.source.Contains('S01')) '未讀到策略原始碼'
    $v1 = Invoke-Tool 60 'strategy_version' @{strategy_id=$fixtureEntry.id; operation='commit'; message='初始策略版本'}
    $v2 = Invoke-Tool 61 'strategy_version' @{strategy_id=$fixtureEntry.id; operation='commit'; message='修改策略參數'; inputs=@{CaseBars=12}}
    $versions = Invoke-Tool 62 'strategy_version' @{strategy_id=$fixtureEntry.id; operation='list'}
    $snapshot = Invoke-Tool 63 'strategy_version' @{strategy_id=$fixtureEntry.id; operation='read'; version=$v2.version}
    Assert-Test ($versions.versions.Count -eq 2 -and $versions.versions[1].version -eq $v1.version -and $snapshot.source -eq $strategy.source -and $snapshot.parameters.inputs.CaseBars -eq 12) '策略版本讀取或參數快照不符'
    $draft = Invoke-Tool 67 'strategy_version' @{strategy_id=$fixtureEntry.id; operation='draft'; version=$v2.version}
    Assert-Test ($draft.id -ne $strategy.id -and $draft.dirty -and !$draft.managed_id -and $draft.source -eq $snapshot.source -and $draft.task.parameters.inputs.CaseBars -eq 12) 'MCP 還原草稿未保留所選版本內容'
    $null = Invoke-Tool 64 'strategy_version' @{strategy_id=$fixtureEntry.id; operation='push'; remote='https://github.com/fixture/example'} -ExpectError
    $null = Invoke-Tool 9 'strategy_check' @{strategy_id=$fixtureEntry.id}
    $null = Invoke-Tool 10 'strategy_check' @{source='if UnknownFunction(Close)>0 then Buy next bar at market;'} -ExpectError
    $logicSource = 'Vars: i(0); if MarketPosition = 1 and MarketPosition = -1 then Buy next bar at market else Buy ("<script>") 0 contracts next bar at market; for i = 1 to 2 begin Sell this bar on close; end;'
    $logic = Invoke-Tool 65 'strategy_logic' @{source=$logicSource; export_html=$true}
    $codes = @($logic.findings.code)
    Assert-Test ($codes -contains 'constant_condition' -and $codes -contains 'invalid_order_quantity' -and $codes -contains 'bar_close_order') '策略靜態檢查未辨識矛盾條件或委託'
    $yes = $logic.edges | Where-Object { $_.from -eq 1 -and $_.to -eq 2 }
    $loopBack = $logic.edges | Where-Object { $_.from -eq 5 -and $_.to -eq 4 }
    Assert-Test ($null -ne $yes -and !$yes.reachable -and $null -ne $loopBack -and $loopBack.reachable) '策略流程缺少不可達分支或迴圈回邊'
    $logicHtml = Get-Content -LiteralPath $logic.report_path -Raw -Encoding UTF8
    Assert-Test ($logicHtml.Contains('&lt;script&gt;') -and !$logicHtml.Contains('<script>') -and $logicHtml.Contains('href="#n3"')) '流程圖未正確標示或跳脫來源文字'
    $diagram = Get-Content -LiteralPath $logic.excalidraw_path -Raw -Encoding UTF8 | ConvertFrom-Json
    $arrows = @($diagram.elements | Where-Object { $_.type -eq 'arrow' })
    $backArrow = @($arrows | Where-Object { $_.startBinding.elementId -eq 'node-5' -and $_.endBinding.elementId -eq 'node-4' })
    Assert-Test ($logicHtml.Contains('href="flow.excalidraw"') -and $diagram.type -eq 'excalidraw' -and $diagram.version -eq 2 -and $arrows.Count -eq $logic.edges.Count -and $backArrow.Count -eq 1) '可編輯流程圖遺漏流程邊或 HTML 下載連結'
    $null = Invoke-Tool 66 'strategy_logic' @{source='if UnknownFunction(Close)>0 then Buy next bar at market;'} -ExpectError
    $baseline = Invoke-Tool 11 'backtest_run' @{strategy_id=$fixtureEntry.id}
    Assert-Test ($baseline.trade_validation.status -eq 'passed' -and $baseline.trade_validation.checked_trades -eq 12) '基準未逐筆驗收'
    $sameBarSource = 'if CurrentBar = 1 then Buy ("SameEntry") next bar at market; if CurrentBar = 2 then Sell ("SameExit") this bar on close;'
    $sameBarResponse = Invoke-Rpc 43 'tools/call' @{name='backtest_run'; arguments=@{strategy_id=$fixtureEntry.id; source=$sameBarSource}}
    $auditId = [regex]::Match($sameBarResponse.result.content[0].text,'rust-\d+-\d+').Value
    Assert-Test ($sameBarResponse.result.isError -and $auditId -and $sameBarResponse.result.content[0].text.Contains('逐筆成交驗收未通過')) '同棒成交不應成為基準'
    $audit = Invoke-Tool 44 'backtest_result' @{run_id=$auditId}
    $violation = $audit.receipt.trade_validation.violations[0]
    Assert-Test ($audit.trade_count -eq 1 -and $audit.receipt.trade_validation.violation_count -eq 1 -and $violation.entry_bar -eq $violation.exit_bar -and $violation.entry_label -eq 'SameEntry' -and $violation.exit_label -eq 'SameExit') '原始成交或同棒違規明細不完整'
    $null = Invoke-Tool 45 'loop_start' @{baseline_run_id=$auditId; goal='驗收'; rubric='逐筆'; max_rounds=1; baseline_score=100; baseline_review='同棒'} -ExpectError
    $report = Invoke-Tool 12 'backtest_result' @{run_id=$baseline.run_id}
    Assert-Test ($report.trade_count -eq 12 -and $baseline.summary.net_closed -eq -8) '人工策略基準績效不符'
    $stressArgs = @{run_id=$baseline.run_id; seed=90210; repetitions=3; noise_ticks=1; block_bars=10}
    $stress = Invoke-Tool 43 'robustness_run' $stressArgs
    $repeat = Invoke-Tool 44 'robustness_run' $stressArgs
    Assert-Test ($stress.groups.ohlc_noise.completed -eq 3 -and $stress.groups.moving_block_bootstrap.completed -eq 3 -and $stress.groups.execution_stress.completed -eq 3) '強健性情境回測失敗'
    $stressFile = Get-Content -LiteralPath $stress.report_path -Raw -Encoding UTF8 | ConvertFrom-Json
    $repeatFile = Get-Content -LiteralPath $repeat.report_path -Raw -Encoding UTF8 | ConvertFrom-Json
    Assert-Test (($stressFile.scenarios | ConvertTo-Json -Depth 12 -Compress) -eq ($repeatFile.scenarios | ConvertTo-Json -Depth 12 -Compress)) '相同種子的情境結果不一致'
    $candidate = Invoke-Tool 13 'optimization_compare' @{baseline_run_id=$baseline.run_id; source='if CurrentBar = 1 then Buy next bar at market; if CurrentBar = 5 then Sell next bar at market;'}
    Assert-Test ($candidate.candidate.summary.trades -eq 1 -and $candidate.delta.trades -eq -11) '候選版交易筆數或差值不符'
    Assert-Test ($candidate.baseline.optimization_inputs.bars -eq $candidate.candidate.optimization_inputs.bars) '行情雜湊不同'
    $null = Invoke-Tool 14 'optimization_compare' @{baseline_run_id=$baseline.run_id; source=$strategy.source} -ExpectError
    $null = Invoke-Tool 15 'backtest_result' @{run_id='../data/market.sqlite'} -ExpectError
    $baselineReport = Invoke-Tool 16 'report_read' @{run_id=$baseline.run_id; include_source=$true}
    Assert-Test ($baselineReport.bar_count -eq 171 -and $baselineReport.source -eq $strategy.source) '結構化報表或回測原碼錯誤'
    $perf = $baselineReport.performance
    Assert-Test ($perf.date_basis -eq 'source_calendar' -and $perf.trading_days -eq 1) '無商品時段快照的回測應以來源曆日統計'
    Assert-Test ($baselineReport.market_coverage.symbol -eq 'REA_SYNTH' -and $baselineReport.market_coverage.first_bar -eq $baselineReport.first_bar -and $baselineReport.market_coverage.last_bar -eq $baselineReport.last_bar -and $baselineReport.market_coverage.status -eq 'not_checked') '回測行情來源、實際起訖或完整性狀態缺漏'
    Assert-Test ($perf.gross_pnl -eq ($perf.net_pnl + $perf.closed_trade_costs) -and $perf.net_pnl -eq $baseline.summary.net_closed -and $perf.trades -eq 12) '完整損益對帳錯誤'
    Assert-Test ($perf.monthly.'2026-10'.net_pnl -eq $perf.net_pnl -and $perf.yearly.'2026'.net_pnl -eq $perf.net_pnl -and $perf.longest_losing_months -eq 1) '期間損益或連虧錯誤'
    foreach ($format in @('html','csv')) {
        $exported = Invoke-Tool 40 'report_read' @{run_id=$baseline.run_id; format=$format}
        $body = Get-Content -LiteralPath $exported.export.path -Raw -Encoding UTF8
        Assert-Test ($exported.export.format -eq $format -and $body.Contains('策略分析') -and $body.Contains('交易明細') -and $body.Contains('週期性分析') -and $body.Contains('補充損益') -and $body.Contains('補充月損益') -and $body.Contains('行情完整性')) '績效匯出欠缺區段'
        $comparison = Invoke-Tool 41 'model_update_report' @{baseline_run_id=$baseline.run_id; candidate_run_id=$candidate.candidate.run_id; format=$format}
        $comparisonBody = Get-Content -LiteralPath $comparison.export.path -Raw -Encoding UTF8
        $netDelta = $comparison.metrics | Where-Object {$_.key -eq 'net_pnl'}
        Assert-Test ($comparison.same_data_and_conditions -and $netDelta.delta -eq $candidate.delta.net_closed -and $comparisonBody.Contains('逐月淨損益')) '模型比較績效差值或匯出錯誤'
        $simple = Invoke-Tool 42 'report_read' @{run_id=$baseline.run_id; format=$format; style='simple'}
        $simpleBody = Get-Content -LiteralPath $simple.export.path -Raw -Encoding UTF8
        Assert-Test ($simple.export.style -eq 'simple') '簡式樣式未生效'
        foreach ($section in @('策略績效總結果','平倉權益曲線','平倉權益曲線及績效拉回','總交易分析','月週期分析','年週期分析','設定')) {
            Assert-Test ($simpleBody.Contains($section)) "簡式報告缺少 $section"
        }
        if ($format -eq 'csv') {
            $r = Import-Csv -LiteralPath $simple.export.path
            $curve = @($r | Where-Object {$_.'區段' -eq '平倉權益曲線' -and !$_.'列號'})
            $drawdowns = @($r | Where-Object {$_.'區段' -eq '平倉權益曲線及績效拉回' -and !$_.'列號'})
            Assert-Test ($curve.Count -eq 14 -and $drawdowns.Count -eq 14 -and [double]$curve[-1].C -eq ($baselineReport.settings.initial_capital + $perf.net_pnl)) '簡式平倉權益未與已平倉淨損益對帳'
            $peak = [double]$curve[1].C
            for ($index = 1; $index -lt $curve.Count; $index++) {
                $current = [double]$curve[$index].C
                $peak = [Math]::Max($peak,$current)
                Assert-Test ([double]$drawdowns[$index].C -eq $current -and [double]$drawdowns[$index].D -eq ($current - $peak)) '簡式平倉回撤未以合併區段的逐筆權益及前高計算'
            }
        }
    }
    $loop = Invoke-Tool 17 'loop_start' @{baseline_run_id=$baseline.run_id; goal='測試研究迴圈'; rubric='人工固定準則';
        max_rounds=2; baseline_score=60; baseline_review='基準 12 筆交易'}
    $loopId = $loop.loop.loop_id
    $invalid = @{loop_id=$loopId; round=1; source=$sameBarSource; hypothesis='測試同棒進出不得進入排名'}
    $rejected = Invoke-Tool 18 'loop_submit' $invalid
    Assert-Test ($rejected.next_round -eq 2 -and $rejected.best.round -eq 0 -and $rejected.loop.rounds[0].error.Contains('逐筆成交驗收未通過')) '未保留拒絕原因或違規候選進入排名'
    $null = Invoke-Tool 46 'loop_score' @{loop_id=$loopId; round=1; score=100; review='違規候選不可評分'} -ExpectError
    $same = Invoke-Tool 19 'loop_submit' $invalid
    Assert-Test ($same.loop.rounds.Count -eq 1) '重送時重複建立候選'
    $process.StandardInput.Close()
    Assert-Test ($process.WaitForExit(5000) -and $process.ExitCode -eq 0) '第一個 MCP 程序未正常結束'
    $firstDiagnostics = $diagnostics.Result
    $process.Dispose()
    $process = [Diagnostics.Process]::Start($start)
    $diagnostics = $process.StandardError.ReadToEndAsync()
    $null = Invoke-Rpc 20 'initialize' @{protocolVersion='2025-11-25'; capabilities=@{}; clientInfo=@{name='resume-test'; version='1'}}
    Send-Message @{jsonrpc='2.0'; method='notifications/initialized'}
    $resumed = Invoke-Tool 21 'loop_status' @{loop_id=$loopId; include_failed_source=$true}
    Assert-Test ($resumed.next_round -eq 2 -and $resumed.last_failed_source -eq $invalid.source) '重開後未還原狀態與失敗原碼'
    $submitted = Invoke-Tool 22 'loop_submit' @{loop_id=$loopId; round=2; hypothesis='以固定持倉期間對照';
        source='if CurrentBar = 1 then Buy next bar at market; if CurrentBar = 5 then Sell next bar at market;'}
    Assert-Test ($submitted.next_action -eq 'score' -and $submitted.awaiting_score -eq 2) '候選回測未進入評分'
    $candidateReport = Invoke-Tool 23 'report_read' @{run_id=$submitted.loop.rounds[1].run_id}
    Assert-Test ($candidateReport.summary.net_closed -eq -2 -and $candidateReport.summary.trades -eq 1) '候選報表績效不符'
    $null = Invoke-Tool 24 'loop_finish' @{loop_id=$loopId; reason='缺少評分'} -ExpectError
    $ranked = Invoke-Tool 25 'loop_score' @{loop_id=$loopId; round=2; score=72; review='淨利及回撤改善，交易樣本仍少'}
    Assert-Test ($ranked.best.round -eq 2 -and $ranked.next_action -eq 'finish') '評分、排名或輪數上限不符'
    $finished = Invoke-Tool 26 'loop_finish' @{loop_id=$loopId; reason='達指定輪數'}
    Assert-Test ($finished.next_action -eq 'finished' -and $finished.best.score -eq 72) '迴圈未保存停止原因或最佳分數'
    $proof = [ordered]@{passed=$true; protocol=$init.result.protocolVersion; tools=$list.result.tools.name;
        baseline=$baseline.run_id; candidate=$candidate.candidate.run_id; delta=$candidate.delta;
        same_bar_rejected_run=$auditId; same_bar_validation=$audit.receipt.trade_validation;
        loop_id=$loopId; resumed=$true; best=$finished.best; api_profiles_shared=$true; api_gateway_profile_loaded=$true}
    $process.StandardInput.Close()
    Assert-Test ($process.WaitForExit(5000) -and $process.ExitCode -eq 0) 'MCP 程序未正常結束'
    $proof | ConvertTo-Json -Depth 6
    Write-Output $out
} finally {
    if (!$process.HasExited) { $process.Kill(); $process.WaitForExit() }
    $encoding = [Text.UTF8Encoding]::new($false)
    [IO.File]::WriteAllText((Join-Path $out 'stderr.log'), ($firstDiagnostics + $diagnostics.Result), $encoding)
    [IO.File]::WriteAllText((Join-Path $out 'verification.json'), ($proof | ConvertTo-Json -Depth 6), $encoding)
    $process.Dispose()
}
