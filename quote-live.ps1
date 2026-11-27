param(
    [string]$SdkDirectory = 'outputs/quotecom-research-20261122/QuoteComExample/bin/Release',
    [string]$Database = 'data/market-live.sqlite',
    [ValidateSet('020','024')][string]$TradeType = '020',
    [ValidateRange(0,6)][Nullable[int]]$TimeDecimals,
    [ValidateSet('raw','decoded')][string]$PriceEncoding,
    [ValidateSet('TXF','MXF','TMF','CDF')][string[]]$Products = @('TXF','MXF','TMF','CDF'),
    [string]$ProfileId = 'KGIFutures',
    [switch]$Nikkei,
    [switch]$BuildOnly
)
$ErrorActionPreference = 'Stop'
Push-Location $PSScriptRoot
try {
    $Instrument = @()
    if (!$Nikkei) {
        if ($null -eq $TimeDecimals) { $TimeDecimals = 2 }
        if (!$PriceEncoding) { $PriceEncoding = 'decoded' }
        $calendar = Join-Path $PSScriptRoot 'data/taifex-monthly-expiries.json'
        if (!(Test-Path -LiteralPath $calendar)) { throw '缺少臺期所最後交易日設定檔。' }
        foreach ($product in $Products | Select-Object -Unique) {
            $Instrument += "--auto-$($product.ToLowerInvariant())=$calendar"
        }
    }
    if (!$BuildOnly -and !$ProfileId) {
        foreach ($name in @('QUOTECOM_CONNECTION','QUOTECOM_HOST','QUOTECOM_PORT','QUOTECOM_SOURCE_ID','QUOTECOM_TOKEN','QUOTECOM_ID','QUOTECOM_PASSWORD')) {
            if ([string]::IsNullOrWhiteSpace([Environment]::GetEnvironmentVariable($name))) {
                throw "缺少 QuoteCom 連線設定 $name"
            }
        }
    }
    $sdk = (Resolve-Path -LiteralPath $SdkDirectory).Path
    $out = Join-Path $PSScriptRoot $(if ($Nikkei) {'outputs/quote-gateway-jpx'} else {'outputs/quote-gateway'})
    New-Item -ItemType Directory -Path $out -Force | Out-Null
    foreach ($name in @('QuoteCom.dll','Package.dll','PushClient.dll','ICSharpCode.SharpZipLib.dll')) {
        Copy-Item -LiteralPath (Join-Path $sdk $name) -Destination $out -Force
    }
    $exe = Join-Path $out 'QuoteGateway.exe'
    $compiler = Join-Path $env:WINDIR 'Microsoft.NET/Framework64/v4.0.30319/csc.exe'
    & $compiler /codepage:65001 /nologo /platform:x64 /r:System.Web.Extensions.dll "/r:$out/QuoteCom.dll" "/r:$out/Package.dll" "/out:$exe" (Join-Path $PSScriptRoot 'native/quote-gateway/QuoteGateway.cs')
    if ($LASTEXITCODE -ne 0) { throw 'QuoteCom 轉接器編譯失敗。' }
    $env:CARGO_HOME = Join-Path $PSScriptRoot '.tools/cargo'
    $env:RUSTUP_HOME = Join-Path $PSScriptRoot '.tools/rustup'
    $buildArgs = @('build','--manifest-path','rust/Cargo.toml','--offline','--bin','live_market')
    if ($Nikkei) { $buildArgs += '--release' }
    & .tools/cargo/bin/cargo.exe @buildArgs
    if ($LASTEXITCODE -ne 0) { throw '行情入庫程式編譯失敗。' }
    if (!$BuildOnly) {
        $profileArguments = @()
        if ($ProfileId) { $profileArguments = @('--profile', $ProfileId) }
        if ($Nikkei) {
            & .\rust\target\release\live_market.exe ([IO.Path]::GetFullPath($Database)) Asia/Tokyo minute --reconnect @profileArguments --gateway $exe jpx225m
        } else {
            & .\rust\target\debug\live_market.exe ([IO.Path]::GetFullPath($Database)) Asia/Taipei taifex --reconnect @profileArguments --gateway $exe $TradeType $TimeDecimals $PriceEncoding @Instrument
        }
        if ($LASTEXITCODE -ne 0) { throw '行情服務已中止，請查看上方原因及資料庫拒收紀錄。' }
    }
} finally { Pop-Location }
