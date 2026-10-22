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
. (Join-Path $PSScriptRoot 'LyFlow-Dependency.ps1')
Assert-RuntimePlatform
$config = Get-RuntimeConfiguration
$projectRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$StageRoot = [IO.Path]::GetFullPath($StageRoot)
$clientTag = Get-LyFlowCargoTag (Join-Path $projectRoot 'src-tauri/Cargo.toml') $config
if ($LyFlowRuntime) {
    $LyFlowRuntime = [IO.Path]::GetFullPath($LyFlowRuntime)
    $explicitDescriptor = Get-Content -LiteralPath (Join-Path $LyFlowRuntime 'lyflow-runtime.json') -Raw | ConvertFrom-Json
    $lyflowVersion = [string](Get-RuntimeField $explicitDescriptor 'engineVersion')
    [void](Test-LyFlowRuntimeDirectory $LyFlowRuntime $config $lyflowVersion)
    Write-Warning "诊断构建使用显式 lyFlow 运行库 $lyflowVersion：$LyFlowRuntime"
} else {
    # 正式打包取系列最新正式版本，网络失败不退回旧缓存，免得把较旧补丁打进安装包
    $restored = Restore-LyFlowDependency $projectRoot $config -NoFallback
    $LyFlowRuntime = $restored.path
    $lyflowVersion = $restored.version
}
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
Assert-LyFlowReleaseMetadata $descriptor '正式产物描述'
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
    lyflow = [ordered]@{ repository = $config.lyflow.repository; series = $config.lyflow.series; version = $descriptor.engineVersion; abiVersion = $descriptor.abiVersion; clientTag = $clientTag; localValidationOnly = $false; operators = $config.requiredOperators }
    mvs = @{ version = $config.mvsVersion }
    files = @(Get-RuntimeFiles $runtimeRoot)
} | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath (Join-Path $runtimeRoot 'manifest.json') -Encoding utf8NoBOM
& (Join-Path $PSScriptRoot 'Test-PackagedRuntime.ps1') -RuntimeRoot $runtimeRoot -ReportPath (Join-Path $StageRoot 'runtime-validation.json')
Write-Host "运行库准备完成：$runtimeRoot"
