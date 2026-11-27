param([switch]$Build,[string]$Database)
$ErrorActionPreference='Stop'
& (Join-Path $PSScriptRoot 'pl-workbench-godot.ps1') -Build:$Build -OpenQuote -Database $Database