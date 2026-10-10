#Requires -Version 7.0
param(
    [string]$LyFlowRuntime = $env:LYFLOW_RUNTIME_DIR,
    [string]$VcRuntime = $env:VC_RUNTIME_DIR
)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
if (-not $IsWindows -or [Runtime.InteropServices.RuntimeInformation]::ProcessArchitecture -ne 'X64') {
    throw '请在 Windows x64 PowerShell 中发包。'
}
$projectRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
if (-not $LyFlowRuntime) { $LyFlowRuntime = Join-Path $projectRoot '../LyFlow/build/core/bin' }
$LyFlowRuntime = (Resolve-Path -LiteralPath $LyFlowRuntime).Path
if (-not (Test-Path -LiteralPath (Join-Path $LyFlowRuntime 'lyflow_core.dll'))) {
    throw '缺少 lyflow_core.dll。请先构建含 glue 算子的 lyFlow，或设置 LYFLOW_RUNTIME_DIR。'
}
if (-not $VcRuntime) {
    $vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio/Installer/vswhere.exe'
    $vsRoot = & $vswhere -latest -products '*' -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
    if ($LASTEXITCODE -ne 0 -or -not $vsRoot) { throw '找不到 Visual C++ 工具链；可通过 VC_RUNTIME_DIR 指定 x64 CRT 目录。' }
    $redistRoot = Join-Path $vsRoot 'VC/Redist/MSVC'
    $crtDirs = @(Get-ChildItem -LiteralPath $redistRoot -Directory | Where-Object { $_.Name -match '^\d+\.\d+\.\d+$' } | Sort-Object { [version]$_.Name } -Descending | ForEach-Object {
        $x64 = Join-Path $_.FullName 'x64'
        if (Test-Path -LiteralPath $x64) { Get-ChildItem -LiteralPath $x64 -Directory -Filter 'Microsoft.VC*.CRT' }
    })
    if (-not $crtDirs.Count) { throw '找不到可分发的 x64 CRT；请设置 VC_RUNTIME_DIR。' }
    $VcRuntime = $crtDirs[0].FullName
}
$VcRuntime = (Resolve-Path -LiteralPath $VcRuntime).Path
foreach ($name in @('msvcp140.dll', 'vcruntime140.dll', 'vcruntime140_1.dll')) {
    if (-not (Test-Path -LiteralPath (Join-Path $VcRuntime $name))) { throw "CRT 目录缺少 $name" }
}
$stageRoot = Join-Path $projectRoot 'src-tauri/target/package-runtime'
$runtimeDir = Join-Path $stageRoot 'lyflow'
foreach ($source in @($LyFlowRuntime, $VcRuntime)) {
    if ($source.StartsWith([IO.Path]::GetFullPath($stageRoot), [StringComparison]::OrdinalIgnoreCase)) {
        throw '运行库来源不能位于会被重新生成的打包暂存目录。'
    }
}
if (Test-Path -LiteralPath $stageRoot) {
    $resolvedStage = (Resolve-Path -LiteralPath $stageRoot).Path
    $expectedStage = [IO.Path]::GetFullPath($stageRoot)
    if ($resolvedStage -ne $expectedStage -or (Get-Item -LiteralPath $stageRoot).LinkType) { throw '拒绝清理重定向的打包目录。' }
    Remove-Item -LiteralPath $stageRoot -Recurse -Force
}
New-Item -ItemType Directory -Path $runtimeDir -Force | Out-Null
foreach ($source in @($LyFlowRuntime, $VcRuntime)) {
    foreach ($dll in Get-ChildItem -LiteralPath $source -Filter '*.dll' -File) {
        Copy-Item -LiteralPath $dll.FullName -Destination (Join-Path $runtimeDir $dll.Name)
    }
}
& (Join-Path $PSScriptRoot 'Test-PackagedRuntime.ps1') -RuntimeDir $runtimeDir
$files = @(Get-ChildItem -LiteralPath $runtimeDir -Filter '*.dll' -File | Sort-Object Name | ForEach-Object {
    @{ name = $_.Name; bytes = $_.Length }
})
@{ architecture = 'x64'; files = $files } | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath (Join-Path $runtimeDir 'manifest.json') -Encoding utf8NoBOM
$configPath = Join-Path $stageRoot 'tauri.runtime.json'
@{ bundle = @{ resources = @{ 'target/package-runtime/lyflow/' = 'runtime/lyflow/' } } } | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath $configPath -Encoding utf8NoBOM
Push-Location -LiteralPath $projectRoot
try {
    Write-Host "已准备 $($files.Count) 个运行库，开始生成 Windows x64 安装包。"
    & pnpm exec tauri build --bundles nsis --target x86_64-pc-windows-msvc --config $configPath
    if ($LASTEXITCODE -ne 0) { throw "Windows 发包失败（退出码 $LASTEXITCODE）" }
} finally { Pop-Location }
