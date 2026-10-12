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
& (Join-Path $PSScriptRoot 'Prepare-PackageResources.ps1')
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
    & (Join-Path $PSScriptRoot 'Test-PackageApplication.ps1') -ApplicationPath $applicationPath -StageRoot $stageRoot
    & pnpm exec tauri bundle --bundles nsis --target x86_64-pc-windows-msvc --config $configPath
    if ($LASTEXITCODE -ne 0) { throw "Windows 安装包生成失败（退出码 $LASTEXITCODE）" }
} finally { Pop-Location }
