#Requires -Version 7.0
param(
    [string]$LyFlowRuntime = $env:LYFLOW_RUNTIME_DIR,
    [string]$MvsRuntime = $env:MVS_RUNTIME_DIR,
    [string]$VcRuntime = $env:VC_RUNTIME_DIR,
    [string]$MvsLicense,
    [string]$OutputDirectory = (Join-Path $PSScriptRoot '../src-tauri/target/dependencies')
)
. (Join-Path $PSScriptRoot 'Runtime-Common.ps1')
$stageRoot = Join-Path $PSScriptRoot '../src-tauri/target/dependency-runtime'
$parameters = @{ StageRoot = $stageRoot }
foreach ($name in @('LyFlowRuntime', 'MvsRuntime', 'VcRuntime', 'MvsLicense')) {
    $value = Get-Variable -Name $name -ValueOnly
    if ($value) { $parameters[$name] = $value }
}
& (Join-Path $PSScriptRoot 'Prepare-RuntimeDependencies.ps1') @parameters
$config = Get-RuntimeConfiguration
[void][IO.Directory]::CreateDirectory([IO.Path]::GetFullPath($OutputDirectory))
Assert-NoRuntimeLinks $OutputDirectory
$archive = Join-Path $OutputDirectory $config.asset
if (Test-Path -LiteralPath $archive) { throw "导出目标已经存在，请移走后重试：$archive" }
Compress-Archive -LiteralPath (Join-Path $stageRoot 'runtime') -DestinationPath $archive -CompressionLevel Optimal
Write-Host "依赖包已导出：$archive"
