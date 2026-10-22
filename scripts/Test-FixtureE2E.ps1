#Requires -Version 7.0
# 【仅旧拍照点配方】特征定位在段模型（分段连拍）下停用，本入口只服务 RecipeDoc.segments 为空的旧配方；旧模型只停用不删除，入口保留。
# 段配方的端到端见 Test-OfflineE2E.ps1（默认 -Model burst）。
# 特征定位示教端到端（tests/native/fixture-teach-e2e.mjs）：隔离 e2e 实例 + 真 lyFlow + 样本集副本，
# 示教页画框（压胶路、高光、好框）→ 定位试测 → 验证统计与建议 → 采用 → 发布 → 离线投产。
param(
    [ValidateRange(1024, 65535)][int]$DebugPort = 9341,
    [switch]$SkipBuild,
    [string]$SampleSet,
    [string]$LyFlowRuntime,
    [string]$DataRoot
)

$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'E2E-Common.ps1')
if (!$IsWindows) { throw '特征定位端到端测试需要 Windows、WebView2 与 lyFlow。' }
$e2eInputs = Resolve-E2EInputs -Title '特征定位端到端' -SampleSet $SampleSet -LyFlowRuntime $LyFlowRuntime
$e2eRun = New-E2ERun -InstanceProfile e2e -DebugPort $DebugPort -DataRoot $DataRoot
$e2eExit = 0

Push-Location -LiteralPath $e2eRun.Root
try {
    Assert-E2ENodeDependencies
    # 只在本进程设运行库目录，不改用户级变量
    $env:GLUESIGHT_LYFLOW_RUNTIME = $e2eInputs.LyFlow
    Start-E2EInstance -Fresh -Prebuilt:$SkipBuild
    & node (Join-Path $e2eRun.Root 'tests\native\fixture-teach-e2e.mjs') `
        --instance $e2eRun.Manifest --output (Join-Path $e2eRun.Output 'fixture\result.json') --sample-set $e2eInputs.SampleSet --lyflow-runtime $e2eInputs.LyFlow
    $e2eExit = $LASTEXITCODE
    if ($e2eExit -ne 0 -and $e2eExit -ne $E2EMissingInputExit) { Write-Error "特征定位端到端测试失败（退出码 $e2eExit）。日志见 $($e2eRun.Output)。" -ErrorAction Continue }
} finally {
    Restore-E2ELyFlowRuntime
    Stop-E2EInstance
    Pop-Location
    Write-Host "特征定位端到端总耗时 $([math]::Round($e2eRun.Watch.Elapsed.TotalSeconds, 1)) s"
}
exit $e2eExit
