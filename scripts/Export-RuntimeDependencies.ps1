#Requires -Version 7.0
param(
    [Parameter(Mandatory)][string]$MvsRuntime,
    [Parameter(Mandatory)][string]$VcRuntime,
    [Parameter(Mandatory)][string]$MvsLicense,
    [string]$OutputDirectory = (Join-Path $PSScriptRoot '../src-tauri/target/dependencies')
)
. (Join-Path $PSScriptRoot 'Vendor-Dependency.ps1')
Assert-RuntimePlatform
$config = Get-RuntimeConfiguration
$stageRoot = Join-Path $PSScriptRoot '../src-tauri/target/vendor-export'
Reset-RuntimeStage $stageRoot @($MvsRuntime, $VcRuntime, $MvsLicense)
$runtimeRoot = Join-Path $stageRoot 'runtime'
Copy-RuntimeTree $MvsRuntime (Join-Path $runtimeRoot 'mvs')
Copy-RuntimeTree $MvsLicense (Join-Path $runtimeRoot 'mvs/Licenses')
Copy-RuntimeTree $VcRuntime (Join-Path $runtimeRoot 'crt') -LyFlow
$versions = [ordered]@{}
foreach ($name in @('msvcp140.dll', 'vcruntime140.dll', 'vcruntime140_1.dll')) {
    $versions[$name] = (Get-Item -LiteralPath (Join-Path $runtimeRoot "crt/$name")).VersionInfo.FileVersion
}
[ordered]@{
    schemaVersion = 2; kind = 'vendor'; bundleVersion = $config.bundleVersion; architecture = 'x64'
    mvs = @{ version = $config.mvsVersion }; crt = @{ fileVersions = $versions }
    files = @(Get-RuntimeFiles $runtimeRoot)
} | ConvertTo-Json -Depth 10 | Set-Content -LiteralPath (Join-Path $runtimeRoot 'manifest.json') -Encoding utf8NoBOM
[void](Assert-VendorRuntime $runtimeRoot)
[void][IO.Directory]::CreateDirectory([IO.Path]::GetFullPath($OutputDirectory))
Assert-NoRuntimeLinks $OutputDirectory
$archive = Join-Path $OutputDirectory $config.asset
if (Test-Path -LiteralPath $archive) { throw "导出目标已存在，禁止覆盖：$archive" }
Compress-Archive -LiteralPath $runtimeRoot -DestinationPath $archive -CompressionLevel Optimal
Write-Host "厂商依赖包已导出：$archive"
