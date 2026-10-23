#Requires -Version 7.0
param(
    [ValidateRange(1024, 65535)][int]$DebugPort = 9339,
    [switch]$SkipBuild,
    [string]$DataRoot,
    [string]$LyFlowRuntime,
    [ValidateSet('burst')][string]$Suite = 'burst',
    [switch]$ResetData,
    [string]$Grep
)
$ErrorActionPreference = 'Stop'
if (!$IsWindows) { throw '桌面落盘测试需要 Windows 和 WebView2。' }
. (Join-Path $PSScriptRoot 'Desktop-Test.ps1')
$runtime = if ($LyFlowRuntime) { $LyFlowRuntime } else { $env:GLUESIGHT_LYFLOW_RUNTIME }
if (!$runtime) {
    $restored = & pwsh -NoProfile -File (Join-Path $PSScriptRoot 'Restore-LyFlowRuntime.ps1')
    if ($LASTEXITCODE -ne 0) { throw 'lyFlow 运行库恢复失败，未执行桌面测试。' }
    $runtime = @($restored)[-1]
    if (!$runtime) { throw 'lyFlow 运行库恢复未返回目录。' }
}
if ($runtime) {
    $runtime = [IO.Path]::GetFullPath($runtime)
    if (!(Test-Path -LiteralPath (Join-Path $runtime 'lyflow_core.dll') -PathType Leaf)) { throw "lyFlow 运行库目录缺少 lyflow_core.dll：$runtime" }
}
$exitCode = Invoke-DesktopTests -DebugPort $DebugPort -DataRoot $DataRoot `
    -SkipBuild:$SkipBuild -ResetData:$ResetData -LyFlowRuntime $runtime -ProductionSuite $Suite -Grep $Grep
exit $exitCode
