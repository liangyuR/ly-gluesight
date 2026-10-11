#Requires -Version 7.0
param(
    [string]$LyFlowRuntime = $env:LYFLOW_RUNTIME_DIR,
    [string]$MvsRuntime = $env:MVS_RUNTIME_DIR,
    [string]$VcRuntime = $env:VC_RUNTIME_DIR,
    [string]$MvsLicense,
    [string]$RuntimeBundle,
    [switch]$PrepareOnly
)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
. (Join-Path $PSScriptRoot 'Runtime-Common.ps1')
$projectRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$stageRoot = Join-Path $projectRoot 'src-tauri/target/package-runtime'
$parameters = @{ StageRoot = $stageRoot }
foreach ($name in @('LyFlowRuntime', 'MvsRuntime', 'VcRuntime', 'MvsLicense', 'RuntimeBundle')) {
    $value = Get-Variable -Name $name -ValueOnly
    if ($value) { $parameters[$name] = $value }
}
& (Join-Path $PSScriptRoot 'Prepare-RuntimeDependencies.ps1') @parameters
if ($PrepareOnly) { return }
$configPath = Join-Path $stageRoot 'tauri.runtime.json'
@{ bundle = @{ resources = @{ 'target/package-runtime/runtime/' = 'runtime/' } } } | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath $configPath -Encoding utf8NoBOM
Push-Location -LiteralPath $projectRoot
try {
    & pnpm exec tauri build --no-bundle --target x86_64-pc-windows-msvc --config $configPath
    if ($LASTEXITCODE -ne 0) { throw "Windows 应用构建失败（退出码 $LASTEXITCODE）" }
    $targetRoot = if ($env:CARGO_TARGET_DIR) {
        if ([IO.Path]::IsPathRooted($env:CARGO_TARGET_DIR)) { [IO.Path]::GetFullPath($env:CARGO_TARGET_DIR) }
        else { [IO.Path]::GetFullPath((Join-Path $projectRoot "src-tauri/$env:CARGO_TARGET_DIR")) }
    } else { Join-Path $projectRoot 'src-tauri/target' }
    $releaseRoot = Join-Path $targetRoot 'x86_64-pc-windows-msvc/release'
    $applicationPath = Join-Path $releaseRoot 'GlueSight.exe'
    if (-not (Test-Path -LiteralPath $applicationPath -PathType Leaf)) { throw "找不到构建后的应用：$applicationPath" }
    Assert-NoRuntimeLinks $releaseRoot -AncestorsOnly
    $installedRuntime = Join-Path $releaseRoot 'runtime'
    $sourceRuntime = Join-Path $stageRoot 'runtime'
    if ([IO.Path]::GetFullPath($installedRuntime).Equals([IO.Path]::GetFullPath($sourceRuntime), [StringComparison]::OrdinalIgnoreCase)) { throw '拒绝覆盖运行库来源目录。' }
    if (Test-Path -LiteralPath $installedRuntime) {
        Assert-NoRuntimeLinks $installedRuntime
        Remove-Item -LiteralPath $installedRuntime -Recurse -Force
    }
    Copy-RuntimeTree $sourceRuntime $installedRuntime
    $applicationReport = Join-Path $stageRoot 'application-runtime-validation.json'
    $start = [Diagnostics.ProcessStartInfo]::new($applicationPath)
    $start.UseShellExecute = $false
    $start.CreateNoWindow = $true
    $start.WorkingDirectory = $stageRoot
    $start.ArgumentList.Add('--check-packaged-runtime')
    $start.ArgumentList.Add($applicationReport)
    $process = [Diagnostics.Process]::Start($start)
    try {
        if (-not $process.WaitForExit(120000)) {
            $process.Kill($true)
            $process.WaitForExit()
            throw '应用运行库自检超过 120 秒；停止生成安装包。'
        }
        if ($process.ExitCode -ne 0) { throw "应用运行库自检失败（退出码 $($process.ExitCode)）；请查看 $applicationReport" }
    } finally { $process.Dispose() }
    if (-not (Test-Path -LiteralPath $applicationReport -PathType Leaf)) { throw '应用运行库自检未生成报告；停止生成安装包。' }
    $result = Get-Content -LiteralPath $applicationReport -Raw | ConvertFrom-Json
    if ($result.status -ne 'ok') { throw "应用运行库自检报告未通过；请查看 $applicationReport" }
    & pnpm exec tauri bundle --bundles nsis --target x86_64-pc-windows-msvc --config $configPath
    if ($LASTEXITCODE -ne 0) { throw "Windows 安装包生成失败（退出码 $LASTEXITCODE）" }
} finally { Pop-Location }
