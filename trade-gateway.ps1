param(
    [string]$SdkDirectory = 'outputs/tradecom-sdk-20251206/TradeComExample/bin/Release'
)
$ErrorActionPreference='Stop'
function Copy-UpdatedFile($Source,$Destination) {
    if(!(Test-Path -LiteralPath $Destination) -or
       (Get-FileHash -LiteralPath $Source).Hash -ne (Get-FileHash -LiteralPath $Destination).Hash){
        Copy-Item -LiteralPath $Source -Destination $Destination -Force
    }
}
Push-Location $PSScriptRoot
try {
    $sdk=(Resolve-Path -LiteralPath $SdkDirectory).Path
    $out=Join-Path $PSScriptRoot 'outputs/trade-gateway'
    New-Item -ItemType Directory -Path $out -Force | Out-Null
    foreach($name in 'TradeCom.dll','KGICGCAPIATL2.dll'){
        $signature=Get-AuthenticodeSignature -LiteralPath (Join-Path $sdk $name)
        if($signature.Status -ne 'Valid' -or $signature.SignerCertificate.Subject -notmatch 'CN=KGI Securities'){
            throw "凱基原廠簽章檢查失敗：$name"
        }
    }
    foreach($name in 'TradeCom.dll','Package.dll','PushClient.dll','ICSharpCode.SharpZipLib.dll','KGICGCAPIATL2.dll'){
        Copy-UpdatedFile (Join-Path $sdk $name) (Join-Path $out $name)
    }
    $manifest=@()
    $signer64=Join-Path $env:CommonProgramFiles 'KGICGCAPIATL2x64.dll'
    if(Test-Path -LiteralPath $signer64){
        $signature=Get-AuthenticodeSignature -LiteralPath $signer64
        if($signature.Status -ne 'Valid' -or $signature.SignerCertificate.Subject -notmatch 'CN=KGI Securities'){
            throw '凱基 64 位元安控元件簽章檢查失敗'
        }
        Copy-UpdatedFile $signer64 (Join-Path $out "KGICGCAPIATL2x64.dll")
        $manifest=@("/win32manifest:$(Join-Path $PSScriptRoot 'native/trade-gateway/TradeGateway.manifest')")
    }
    $compiler=Join-Path $env:WINDIR 'Microsoft.NET/Framework64/v4.0.30319/csc.exe'
    & $compiler @manifest /codepage:65001 /nologo /platform:x64 /r:System.Windows.Forms.dll /r:System.Web.Extensions.dll "/r:$out/TradeCom.dll" "/r:$out/Package.dll" "/out:$out/TradeGateway.exe" (Join-Path $PSScriptRoot 'native/trade-gateway/TradeGateway.cs')
    if($LASTEXITCODE -ne 0){throw 'TradeCom 轉接器編譯失敗'}
    Write-Output (Join-Path $out 'TradeGateway.exe')
} finally { Pop-Location }
