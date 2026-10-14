#Requires -Version 7.0
param(
    [string]$Series,
    [string]$RepositoryRoot = (Join-Path $PSScriptRoot '..')
)
. (Join-Path $PSScriptRoot 'LyFlow-Dependency.ps1')
Assert-RuntimePlatform
$RepositoryRoot = [IO.Path]::GetFullPath($RepositoryRoot)
$config = Get-LyFlowDependencyConfig $RepositoryRoot
$previousSeries = $config.lyflow.series
$previousTag = Get-LyFlowCargoTag (Join-Path $RepositoryRoot 'src-tauri/Cargo.toml') $config
if ($Series) {
    if ($Series -cnotmatch '^(0|[1-9]\d*)\.(0|[1-9]\d*)$') { throw "-Series 必须写成 主.次：$Series" }
    $config.lyflow.series = $Series
}
$runtime = Restore-LyFlowDependency $RepositoryRoot $config -NoFallback
$tag = "v$($runtime.version)"
Set-LyFlowDependencyFiles $RepositoryRoot $config.lyflow.series $tag
Write-Host "lyFlow 依赖已更新：系列 $previousSeries → $($config.lyflow.series)，lyflow-client $previousTag → $tag，运行库 $($runtime.path)"
