#Requires -Version 7.0
$ErrorActionPreference = 'Stop'
$root = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$targetRoot = if ($env:CARGO_TARGET_DIR) {
    if ([IO.Path]::IsPathRooted($env:CARGO_TARGET_DIR)) { [IO.Path]::GetFullPath($env:CARGO_TARGET_DIR) }
    else { [IO.Path]::GetFullPath((Join-Path $root "src-tauri/$env:CARGO_TARGET_DIR")) }
} else { Join-Path $root 'src-tauri/target' }
$debugRuntime = Join-Path $targetRoot 'debug/runtime/lyflow'

. (Join-Path $PSScriptRoot 'LyFlow-Dependency.ps1')
Assert-RuntimePlatform
$runtime = Restore-LyFlowDependency $root (Get-LyFlowDependencyConfig $root)
if (Test-Path -LiteralPath $debugRuntime) { Remove-Item -LiteralPath $debugRuntime -Recurse -Force }
[void][IO.Directory]::CreateDirectory($debugRuntime)
foreach ($item in Get-ChildItem -LiteralPath $runtime.path -Force) { Copy-Item -LiteralPath $item.FullName -Destination $debugRuntime -Recurse -Force }
Write-Host "lyFlow $($runtime.version)（来源 $($runtime.source)）已复制到 $debugRuntime"
