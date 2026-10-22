#Requires -Version 7.0
param(
    [ValidateRange(1024, 65535)][int]$DebugPort = 9341,
    [switch]$SkipBuild,
    [string]$SampleSet,
    [string]$LyFlowRuntime,
    [string]$DataRoot,
    [ValidateRange(0, 10000)][int]$ArmBudgetMs = 0,
    # burst = 段配方（分段连拍，默认）：合成图三相机主流程 + 样本集冒烟；legacy = 旧拍照点配方（只停用不删除，行为不变）
    [ValidateSet('burst', 'legacy')][string]$Model = 'burst'
)

$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'E2E-Common.ps1')
if (!$IsWindows) { throw '离线端到端测试需要 Windows、WebView2 与 lyFlow。' }
$e2eInputs = Resolve-E2EInputs -Title '离线端到端' -SampleSet $SampleSet -LyFlowRuntime $LyFlowRuntime
$e2eRun = New-E2ERun -InstanceProfile e2e -DebugPort $DebugPort -DataRoot $DataRoot
$e2eExit = 0

Push-Location -LiteralPath $e2eRun.Root
try {
    Assert-E2ENodeDependencies
    $env:GLUESIGHT_LYFLOW_RUNTIME = $e2eInputs.LyFlow
    $e2eResult = Join-Path $e2eRun.Output 'result.json'
    $e2eArmArgs = if ($ArmBudgetMs -gt 0) { @('--arm-ms', $ArmBudgetMs) } else { @() }
    Start-E2EInstance -Fresh -Prebuilt:$SkipBuild
    & node (Join-Path $e2eRun.Root 'tests\native\offline-e2e.mjs') `
        --instance $e2eRun.Manifest --output $e2eResult --sample-set $e2eInputs.SampleSet --lyflow-runtime $e2eInputs.LyFlow --model $Model @e2eArmArgs
    $e2eExit = $LASTEXITCODE
    if ($e2eExit -eq 0) {
        Stop-E2EInstance
        Start-Sleep -Seconds 2
        Start-E2EInstance -Prebuilt
        & node (Join-Path $e2eRun.Root 'tests\native\offline-restart.mjs') `
            --instance $e2eRun.Manifest --previous $e2eResult --output (Join-Path $e2eRun.Output 'restart.json')
        $e2eExit = $LASTEXITCODE
    }
    if ($e2eExit -ne 0 -and $e2eExit -ne $E2EMissingInputExit) { Write-Error "离线端到端测试失败（退出码 $e2eExit）。日志见 $($e2eRun.Output)。" -ErrorAction Continue }
} finally {
    Restore-E2ELyFlowRuntime
    Stop-E2EInstance
    Pop-Location
    Write-Host "离线端到端总耗时 $([math]::Round($e2eRun.Watch.Elapsed.TotalSeconds, 1)) s"
}
exit $e2eExit
