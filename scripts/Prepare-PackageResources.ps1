#Requires -Version 7.0
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
@{ bundle = @{ resources = $resources } } | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath (Join-Path $stageRoot 'tauri.runtime.json') -Encoding utf8NoBOM
