param([switch]$Build,[switch]$OpenQuote,[string]$Database,[switch]$OpenApi,[switch]$OpenTradeCom,[switch]$OpenSettings,[switch]$OpenMixRouteKey)
$ErrorActionPreference='Stop'
Push-Location $PSScriptRoot
try {
    $engine=Join-Path $PSScriptRoot '.tools/godot/Godot_v4.7.2-stable_win64.exe'
    if(!(Test-Path -LiteralPath $engine)){throw '找不到專案內Godot執行檔'}
    $dll=Join-Path $PSScriptRoot 'godot-workbench/bin/recharts_core-ide-diff.dll'
    if($Build -or !(Test-Path -LiteralPath $dll) -or !(Test-Path -LiteralPath 'rust/target/debug/backtest.exe') -or !(Test-Path -LiteralPath 'rust/target/debug/strategy_mcp.exe')) {
        $env:CARGO_HOME=Join-Path $PSScriptRoot '.tools/cargo'
        $env:RUSTUP_HOME=Join-Path $PSScriptRoot '.tools/rustup'
        & .tools/cargo/bin/cargo.exe build --manifest-path rust/Cargo.toml --release --lib --offline
        if($LASTEXITCODE -ne 0){throw 'Rust工作台模組編譯失敗'}
        Copy-Item -LiteralPath 'rust/target/release/recharts_core.dll' -Destination $dll -Force
        & .tools/cargo/bin/cargo.exe build --manifest-path rust/Cargo.toml --bin backtest --bin pl_workbench_cli --bin strategy_mcp --bin trading --offline
        if($LASTEXITCODE -ne 0){throw '回測核心編譯失敗'}
    }
    # 從檔案總管啟動時可以沿用 Codex 隨附的 Git。
    if (!(Get-Command git.exe -CommandType Application -ErrorAction SilentlyContinue)) {
        $gitRuntime = Get-AppxPackage -Name 'OpenAI.CodexPrimaryRuntime*' -ErrorAction SilentlyContinue |
            Sort-Object Version -Descending |
            ForEach-Object { Join-Path $_.InstallLocation 'dependencies/native/git/cmd' } |
            Where-Object { Test-Path -LiteralPath (Join-Path $_ 'git.exe') -PathType Leaf } |
            Select-Object -First 1
        if ($gitRuntime) { $env:Path = $gitRuntime + [IO.Path]::PathSeparator + $env:Path }
    }
    $env:APPDATA=Join-Path $PSScriptRoot '.tools/appdata'
    $project=Join-Path $PSScriptRoot 'godot-workbench'
    if(!(Test-Path -LiteralPath 'godot-workbench/.godot/extension_list.cfg')) {
        & .tools/godot/Godot_v4.7.2-stable_win64_console.exe --headless --path $project --editor --import --quit
        if($LASTEXITCODE -ne 0){throw 'Godot工作台初始化失敗'}
    }
    $arguments='--path "'+$project+'"'
    if($OpenMixRouteKey) {
        $arguments+=' -- --open-mixroute-key'
    } elseif($OpenSettings) {
        $arguments+=' -- --open-settings'
    } elseif($OpenTradeCom) {
        $arguments+=' -- --open-tradecom-settings'
    } elseif($OpenApi) {
        $arguments+=' -- --open-api-settings'
    } elseif($OpenQuote -or $Database) {
        $arguments+=' -- --open-quote'
        if($Database){$arguments+=' --database="'+[IO.Path]::GetFullPath($Database)+'"'}
    }
    Start-Process -FilePath $engine -ArgumentList $arguments -WorkingDirectory $PSScriptRoot -WindowStyle Normal
} finally {Pop-Location}
