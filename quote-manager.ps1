param([switch]$Build,[string]$Database,[switch]$Legacy)
$ErrorActionPreference='Stop'
if(!$Legacy) {
    & (Join-Path $PSScriptRoot 'quote-manager-godot.ps1') -Build:$Build -Database $Database
    return
}
Push-Location $PSScriptRoot
try {
    if ($Build -or !(Test-Path -LiteralPath 'rust/target/release/quote_manager_gui.exe')) {
        $env:CARGO_HOME=Join-Path $PSScriptRoot '.tools/cargo'
        $env:RUSTUP_HOME=Join-Path $PSScriptRoot '.tools/rustup'
        & .tools/cargo/bin/cargo.exe build --manifest-path rust/Cargo.toml --release --bin quote_manager --bin quote_manager_gui --offline
        if($LASTEXITCODE -ne 0){throw '行情資料管理編譯失敗'}
    }
    $options=@{FilePath=(Join-Path $PSScriptRoot 'rust/target/release/quote_manager_gui.exe');WorkingDirectory=$PSScriptRoot;WindowStyle='Normal'}
    if($Database){$options.ArgumentList='--database="'+$Database+'"'}
    Start-Process @options
} finally {Pop-Location}
