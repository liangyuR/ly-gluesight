#Requires -Version 7.0
param(
    [string]$RepositoryRoot = (Join-Path $PSScriptRoot '..'),
    [string]$RuntimeConfigPath,
    [string]$OutputFile
)
. (Join-Path $PSScriptRoot 'LyFlow-Dependency.ps1')
Assert-RuntimePlatform
$RepositoryRoot = [IO.Path]::GetFullPath($RepositoryRoot)
$config = Get-LyFlowDependencyConfig $RepositoryRoot $RuntimeConfigPath
$tag = Get-LyFlowCargoTag (Join-Path $RepositoryRoot 'src-tauri/Cargo.toml') $config
$runtime = Restore-LyFlowDependency $RepositoryRoot $config
Write-Host "lyFlow 运行库 $($runtime.version)（系列 $($config.lyflow.series)，客户端 $tag，来源 $($runtime.source)）：$($runtime.path)"
if ($OutputFile) { [IO.File]::WriteAllText([IO.Path]::GetFullPath($OutputFile), ($runtime | ConvertTo-Json -Compress)) }
$runtime.path
