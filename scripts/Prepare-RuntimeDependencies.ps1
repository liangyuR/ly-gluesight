#Requires -Version 7.0
param(
    [string]$LyFlowRuntime = $env:LYFLOW_RUNTIME_DIR,
    [string]$MvsRuntime = $env:MVS_RUNTIME_DIR,
    [string]$VcRuntime = $env:VC_RUNTIME_DIR,
    [string]$MvsLicense,
    [string]$VendorBundle,
    [string]$StageRoot = (Join-Path $PSScriptRoot '../src-tauri/target/package-runtime')
)
. (Join-Path $PSScriptRoot 'Vendor-Dependency.ps1')
Assert-RuntimePlatform
$config = Get-RuntimeConfiguration
$StageRoot = [IO.Path]::GetFullPath($StageRoot)
if (-not $LyFlowRuntime) { $LyFlowRuntime = & (Join-Path $PSScriptRoot 'Restore-LyFlowRuntime.ps1') }
$vendor = $null
if ($VendorBundle) {
    if ($MvsRuntime -or $VcRuntime -or $MvsLicense) { throw 'VendorBundle 不能与厂商运行库路径参数或环境变量混用。' }
    $vendor = if (Test-Path -LiteralPath (Join-Path $VendorBundle 'runtime')) { Join-Path $VendorBundle 'runtime' } else { $VendorBundle }
    [void](Assert-VendorRuntime $vendor)
} elseif (-not ($MvsRuntime -or $VcRuntime -or $MvsLicense)) {
    $vendor = Restore-VendorRuntime
}
if ($vendor) {
    $MvsRuntime = Join-Path $vendor 'mvs'
    $VcRuntime = Join-Path $vendor 'crt'
    $MvsLicense = Join-Path $MvsRuntime 'Licenses'
}
if (-not $MvsRuntime -or -not $VcRuntime -or -not $MvsLicense) { throw '诊断构建必须显式提供 MvsRuntime、VcRuntime 和 MvsLicense；不自动查找系统 SDK。' }
$descriptorPath = Join-Path $LyFlowRuntime 'lyflow-runtime.json'
if (-not (Test-Path -LiteralPath $descriptorPath -PathType Leaf)) { throw '正式 lyFlow 产物缺少 lyflow-runtime.json。' }
$descriptor = Get-Content -LiteralPath $descriptorPath -Raw | ConvertFrom-Json
Assert-NoRuntimeDigests $descriptor
Assert-LyFlowVersions $descriptor.abiVersion $descriptor.engineVersion '正式产物描述'
Assert-LyFlowReleaseMetadata $descriptor '正式产物描述'
Assert-LyFlowSourceRevision $descriptor.sourceRevision $config.lyflowSourceRevision '正式产物与锁定记录'
Assert-MvsRuntimeStructure $MvsRuntime
if (-not @(Get-ChildItem -LiteralPath $MvsLicense -File -Recurse).Count) { throw '缺少 MVS 许可证。' }
foreach ($name in @('msvcp140.dll', 'vcruntime140.dll', 'vcruntime140_1.dll')) { Assert-X64RuntimeImage (Join-Path $VcRuntime $name) }
Reset-RuntimeStage $StageRoot @($LyFlowRuntime, $MvsRuntime, $MvsLicense, $VcRuntime)
$runtimeRoot = Join-Path $StageRoot 'runtime'
Copy-RuntimeTree $LyFlowRuntime (Join-Path $runtimeRoot 'lyflow') -LyFlow
Copy-Item -LiteralPath $descriptorPath -Destination (Join-Path $runtimeRoot 'lyflow/lyflow-runtime.json')
Copy-RuntimeTree $VcRuntime (Join-Path $runtimeRoot 'crt') -LyFlow
Copy-RuntimeTree $MvsRuntime (Join-Path $runtimeRoot 'mvs')
Copy-RuntimeTree $MvsLicense (Join-Path $runtimeRoot 'mvs/Licenses')
[ordered]@{
    schemaVersion = 2; bundleVersion = $config.bundleVersion; architecture = $config.architecture
    lyflow = @{ packageId = $config.lyflow.packageId; version = $descriptor.engineVersion; abiVersion = $descriptor.abiVersion; sourceRevision = $descriptor.sourceRevision; localValidationOnly = $false; operators = $config.requiredOperators }
    mvs = @{ version = $config.mvsVersion }
    files = @(Get-RuntimeFiles $runtimeRoot)
} | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath (Join-Path $runtimeRoot 'manifest.json') -Encoding utf8NoBOM
& (Join-Path $PSScriptRoot 'Test-PackagedRuntime.ps1') -RuntimeRoot $runtimeRoot -ReportPath (Join-Path $StageRoot 'runtime-validation.json')
Write-Host "运行库准备完成：$runtimeRoot"
