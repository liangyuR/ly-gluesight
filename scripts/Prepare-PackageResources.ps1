#Requires -Version 7.0
# Version：写进 tauri 配置的应用版本（预发布时为 X.Y.Z-pre.N），不传沿用 tauri.conf.json
param([string]$Version)
. (Join-Path $PSScriptRoot 'Runtime-Common.ps1')
$stageRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '../src-tauri/target/package-runtime'))
$applicationRuntimeRoot = Join-Path $stageRoot 'application-runtime'
[void][IO.Directory]::CreateDirectory($applicationRuntimeRoot)
$resources = @{ 'target/package-runtime/runtime/' = 'runtime/' }
foreach ($name in @('vcruntime140.dll', 'vcruntime140_1.dll')) {
    $source = Join-Path $stageRoot "runtime/crt/$name"
    Assert-X64RuntimeImage $source
    Copy-Item -LiteralPath $source -Destination (Join-Path $applicationRuntimeRoot $name)
    $resources["target/package-runtime/application-runtime/$name"] = $name
}
$config = @{ bundle = @{ resources = $resources } }
if ($Version) { $config.version = $Version }
$config | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath (Join-Path $stageRoot 'tauri.runtime.json') -Encoding utf8NoBOM
