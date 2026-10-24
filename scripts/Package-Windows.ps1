#Requires -Version 7.0
param(
    [string]$LyFlowRuntime = $env:LYFLOW_RUNTIME_DIR,
    [string]$MvsRuntime = $env:MVS_RUNTIME_DIR,
    [string]$VcRuntime = $env:VC_RUNTIME_DIR,
    [string]$MvsLicense,
    [string]$VendorBundle,
    [switch]$PrepareOnly,
    # 预发布：版本编成 X.Y.Z-pre.N（N 按 dist-installers 里同版本已有的最大序号自动加 1），写进程序、日志与工件记录；源码版本号不变
    [switch]$Pre
)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
. (Join-Path $PSScriptRoot 'Runtime-Common.ps1')
$projectRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$timings = [ordered]@{}
$clock = [Diagnostics.Stopwatch]::StartNew()
function Complete-Stage([string]$Name) { $timings[$Name] = [math]::Round($clock.Elapsed.TotalSeconds, 1); $clock.Restart(); Write-Host "[$Name] $($timings[$Name]) s" }
$baseVersion = (& (Join-Path $PSScriptRoot 'Test-PackageVersions.ps1') | ConvertFrom-Json).application
$archiveRoot = Join-Path $projectRoot 'dist-installers'
$version = $baseVersion
if ($Pre) {
    $taken = @(Get-ChildItem -LiteralPath $archiveRoot -File -Filter "GlueSight_$baseVersion-pre.*_x64-setup.exe" -ErrorAction SilentlyContinue |
        ForEach-Object { if ($_.Name -match '-pre\.(\d+)_') { [int]$Matches[1] } })
    $version = "$baseVersion-pre.$((@($taken) + 0 | Measure-Object -Maximum).Maximum + 1)"
}
Write-Host "打包版本 $version"
$stageRoot = Join-Path $projectRoot 'src-tauri/target/package-runtime'
$parameters = @{ StageRoot = $stageRoot }
foreach ($name in @('LyFlowRuntime', 'MvsRuntime', 'VcRuntime', 'MvsLicense', 'VendorBundle')) {
    $value = Get-Variable -Name $name -ValueOnly
    if ($value) { $parameters[$name] = $value }
}
& (Join-Path $PSScriptRoot 'Prepare-RuntimeDependencies.ps1') @parameters
Complete-Stage 'runtime'
if ($PrepareOnly) { return }
$configPath = Join-Path $stageRoot 'tauri.runtime.json'
& (Join-Path $PSScriptRoot 'Prepare-PackageResources.ps1') -Version $version
Push-Location -LiteralPath $projectRoot
try {
    # 预发布开 release 增量编译：改动后重编从约 4 分钟降到 1 分钟内，优化略弱；正式版显式关闭，不受外部环境变量影响
    $env:CARGO_PROFILE_RELEASE_INCREMENTAL = if ($Pre) { 'true' } else { 'false' }
    & node (Join-Path $projectRoot 'node_modules/@tauri-apps/cli/tauri.js') build --no-bundle --target x86_64-pc-windows-msvc --config $configPath -- --locked
    if ($LASTEXITCODE -ne 0) { throw "Windows 应用构建失败（退出码 $LASTEXITCODE）" }
    Complete-Stage 'build'
    $targetRoot = if ($env:CARGO_TARGET_DIR) {
        if ([IO.Path]::IsPathRooted($env:CARGO_TARGET_DIR)) { [IO.Path]::GetFullPath($env:CARGO_TARGET_DIR) }
        else { [IO.Path]::GetFullPath((Join-Path $projectRoot "src-tauri/$env:CARGO_TARGET_DIR")) }
    } else { Join-Path $projectRoot 'src-tauri/target' }
    $releaseRoot = Join-Path $targetRoot 'x86_64-pc-windows-msvc/release'
    $applicationPath = Join-Path $releaseRoot 'GlueSight.exe'
    & (Join-Path $PSScriptRoot 'Test-PackageApplication.ps1') -ApplicationPath $applicationPath -StageRoot $stageRoot
    Complete-Stage 'app-check'
    # 旧版本的安装包留在输出目录会让下面“只有一个安装包”的检查失败；历史安装包在 dist-installers
    $bundleRoot = Join-Path $releaseRoot 'bundle/nsis'
    Get-ChildItem -LiteralPath $bundleRoot -File -Filter '*-setup.exe' -ErrorAction SilentlyContinue | Remove-Item -Force
    & node (Join-Path $projectRoot 'node_modules/@tauri-apps/cli/tauri.js') bundle --bundles nsis --target x86_64-pc-windows-msvc --config $configPath
    if ($LASTEXITCODE -ne 0) { throw "Windows 安装包生成失败（退出码 $LASTEXITCODE）" }
    Complete-Stage 'bundle'
    $installers = @(Get-ChildItem -LiteralPath $bundleRoot -File -Filter '*-setup.exe')
    if ($installers.Count -ne 1 -or $installers[0].Name -cne "GlueSight_${version}_x64-setup.exe") { throw "必须只有一个 GlueSight_${version}_x64-setup.exe 安装包。" }
    $reportRoot = Join-Path $projectRoot 'src-tauri/target/package-reports'
    [void][IO.Directory]::CreateDirectory($reportRoot)
    [void][IO.Directory]::CreateDirectory($archiveRoot)
    Copy-Item -LiteralPath $installers[0].FullName -Destination $archiveRoot -Force
    $timingPath = Join-Path $reportRoot 'stage-seconds.txt'
    $timings.GetEnumerator() | ForEach-Object { "$($_.Key)=$($_.Value)" } | Set-Content -LiteralPath $timingPath -Encoding utf8NoBOM
    & (Join-Path $PSScriptRoot 'Write-ReleaseManifest.ps1') -Status 'success' -TimingPath $timingPath -Version $version
    Write-Host "安装包：$(Join-Path $archiveRoot $installers[0].Name)"
    Write-Host "各阶段耗时（秒）：$(($timings.GetEnumerator() | ForEach-Object { "$($_.Key) $($_.Value)" }) -join '，')"
} finally { Pop-Location }
