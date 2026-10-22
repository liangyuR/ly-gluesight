#Requires -Version 7.0
param(
    [ValidateRange(1024, 65535)][int]$DebugPort = 9346,
    [switch]$SkipBuild,
    [string]$SampleSet,
    [string]$LyFlowRuntime,
    [string]$DataRoot,
    [ValidateRange(3, 50)][int]$Parts = 3
)

$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'E2E-Common.ps1')
if (!$IsWindows) { throw '离线回放验证需要 Windows、WebView2 与 lyFlow。' }
$replayInputs = Resolve-E2EInputs -Title '离线回放验证' -SampleSet $SampleSet -LyFlowRuntime $LyFlowRuntime
# 实例与离线端到端共用 e2e 配置（实例日志在 e2e-regression），回放报告单独输出到 log-replay。
$replayRun = New-E2ERun -InstanceProfile e2e -DebugPort $DebugPort -DataRoot $DataRoot
$replayOutput = Join-Path $replayRun.Root 'output\playwright\log-replay'
$replayExit = 0

Push-Location -LiteralPath $replayRun.Root
try {
    Assert-E2ENodeDependencies
    $env:GLUESIGHT_LYFLOW_RUNTIME = $replayInputs.LyFlow
    Start-E2EInstance -Fresh -Prebuilt:$SkipBuild
    & node (Join-Path $replayRun.Root 'tests\native\log-replay-offline.mjs') `
        --instance $replayRun.Manifest --sample-set $replayInputs.SampleSet --output $replayOutput --parts $Parts
    $replayExit = $LASTEXITCODE
    if ($replayExit -ne 0) { Write-Error "离线回放验证失败（退出码 $replayExit）。日志见 $replayOutput 与 $($replayRun.Output)。" -ErrorAction Continue }
} finally {
    Restore-E2ELyFlowRuntime
    Stop-E2EInstance
    Pop-Location
    Write-Host "离线回放验证总耗时 $([math]::Round($replayRun.Watch.Elapsed.TotalSeconds, 1)) s"
}
exit $replayExit
