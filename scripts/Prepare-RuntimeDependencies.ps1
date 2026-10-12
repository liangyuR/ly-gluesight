#Requires -Version 7.0
param(
    [string]$LyFlowRuntime = $env:LYFLOW_RUNTIME_DIR,
    [string]$MvsRuntime = $env:MVS_RUNTIME_DIR,
    [string]$VcRuntime = $env:VC_RUNTIME_DIR,
    [string]$MvsLicense,
    [string]$RuntimeBundle,
    [string]$StageRoot = (Join-Path $PSScriptRoot '../src-tauri/target/package-runtime')
)
. (Join-Path $PSScriptRoot 'Runtime-Common.ps1')
Assert-RuntimePlatform
$config = Get-RuntimeConfiguration
$StageRoot = [IO.Path]::GetFullPath($StageRoot)
if ($RuntimeBundle) {
    if ($LyFlowRuntime -or $MvsRuntime -or $VcRuntime -or $MvsLicense) { throw 'RuntimeBundle 不能与本机运行库来源参数或环境变量同时使用。' }
    $sourceRoot = (Resolve-Path -LiteralPath (Join-Path $RuntimeBundle 'runtime')).Path
    [void](Assert-RuntimeManifest $sourceRoot)
    Reset-RuntimeStage $StageRoot @($sourceRoot)
    Copy-RuntimeTree $sourceRoot (Join-Path $StageRoot 'runtime')
} else {
    if (-not $LyFlowRuntime) { $LyFlowRuntime = Join-Path $PSScriptRoot '../../LyFlow/build/core/bin' }
    if (-not $MvsRuntime) { $MvsRuntime = Join-Path ${env:CommonProgramFiles(x86)} 'MVS/Runtime/Win64_x64' }
    if (-not (Test-Path -LiteralPath (Join-Path $LyFlowRuntime 'lyflow_core.dll'))) { throw '缺少正式 lyflow_core.dll；请通过 -LyFlowRuntime 或 LYFLOW_RUNTIME_DIR 指定正式产物目录。' }
    $descriptorPath = Join-Path $LyFlowRuntime 'lyflow-runtime.json'
    if (-not (Test-Path -LiteralPath $descriptorPath)) { throw '正式 lyFlow 产物缺少 lyflow-runtime.json（abiVersion、engineVersion、sourceRevision）。' }
    $descriptor = Get-Content -LiteralPath $descriptorPath -Raw | ConvertFrom-Json
    Assert-NoRuntimeDigests $descriptor
    Assert-LyFlowVersions $descriptor.abiVersion $descriptor.engineVersion '正式产物描述'
    Assert-LyFlowReleaseMetadata $descriptor '正式产物描述'
    if (-not (Test-Path -LiteralPath (Join-Path $MvsRuntime 'MvCameraControl.dll'))) { throw 'MVS 目录缺少 MvCameraControl.dll；请设置 MVS_RUNTIME_DIR。' }
    if (-not $MvsLicense) {
        $candidates = @((Join-Path $MvsRuntime 'Licenses'), (Join-Path $MvsRuntime '../../Licenses'), (Join-Path $MvsRuntime 'License'), (Join-Path $MvsRuntime '../License'), (Join-Path $MvsRuntime '../../License'), (Join-Path ${env:ProgramFiles(x86)} 'MVS/License'))
        $MvsLicense = $candidates | Where-Object { Test-Path -LiteralPath $_ -PathType Container } | Select-Object -First 1
    }
    if (-not $MvsLicense -or -not @(Get-ChildItem -LiteralPath $MvsLicense -File -Recurse).Count) { throw '缺少 MVS 许可证目录；请通过 -MvsLicense 指定官方 License 目录。' }
    if (-not $VcRuntime) {
        $vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio/Installer/vswhere.exe'
        if (-not (Test-Path -LiteralPath $vswhere)) { throw '找不到 vswhere；请设置 VC_RUNTIME_DIR 指向 x64 CRT 目录。' }
        $vsRoot = & $vswhere -latest -products '*' -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
        if ($LASTEXITCODE -ne 0 -or -not $vsRoot) { throw '找不到 Visual C++ 工具链；请设置 VC_RUNTIME_DIR。' }
        $crtDirs = @(Get-ChildItem -LiteralPath (Join-Path $vsRoot 'VC/Redist/MSVC') -Directory | Where-Object Name -Match '^\d+\.\d+\.\d+$' | Sort-Object { [version]$_.Name } -Descending | ForEach-Object {
            $x64 = Join-Path $_.FullName 'x64'
            if (Test-Path -LiteralPath $x64) { Get-ChildItem -LiteralPath $x64 -Directory -Filter 'Microsoft.VC*.CRT' }
        })
        if (-not $crtDirs.Count) { throw '找不到可分发的 x64 CRT；请设置 VC_RUNTIME_DIR。' }
        $VcRuntime = $crtDirs[0].FullName
    }
    foreach ($name in @('msvcp140.dll', 'vcruntime140.dll', 'vcruntime140_1.dll')) {
        if (-not (Test-Path -LiteralPath (Join-Path $VcRuntime $name))) { throw "CRT 目录缺少 $name" }
    }
    Reset-RuntimeStage $StageRoot @($LyFlowRuntime, $MvsRuntime, $MvsLicense, $VcRuntime)
    $runtimeRoot = Join-Path $StageRoot 'runtime'
    Copy-RuntimeTree $LyFlowRuntime (Join-Path $runtimeRoot 'lyflow') -LyFlow
    Copy-Item -LiteralPath $descriptorPath -Destination (Join-Path $runtimeRoot 'lyflow/lyflow-runtime.json')
    Copy-RuntimeTree $VcRuntime (Join-Path $runtimeRoot 'lyflow') -LyFlow
    Copy-RuntimeTree $MvsRuntime (Join-Path $runtimeRoot 'mvs')
    Copy-RuntimeTree $MvsLicense (Join-Path $runtimeRoot 'mvs/Licenses')
    [ordered]@{
        bundleVersion = $config.bundleVersion; architecture = $config.architecture
        lyflow = @{ version = $descriptor.engineVersion; abiVersion = $descriptor.abiVersion; sourceRevision = $descriptor.sourceRevision; localValidationOnly = $false; operators = $config.requiredOperators }
        mvs = @{ version = $config.mvsVersion }
        files = @(Get-RuntimeFiles $runtimeRoot)
    } | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath (Join-Path $runtimeRoot 'manifest.json') -Encoding utf8NoBOM
}
$runtimeRoot = Join-Path $StageRoot 'runtime'
& (Join-Path $PSScriptRoot 'Test-PackagedRuntime.ps1') -RuntimeRoot $runtimeRoot -ReportPath (Join-Path $StageRoot 'runtime-validation.json')
Write-Host "运行库准备完成：$runtimeRoot"
