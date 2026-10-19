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
$replayMissingInputExit = 3
if (!$IsWindows) { throw '离线回放验证需要 Windows、WebView2 与 lyFlow。' }
$replaySampleSet = if ($SampleSet) { $SampleSet } else { $env:GLUESIGHT_SAMPLE_SET }
$replayLyflow = if ($LyFlowRuntime) { [IO.Path]::GetFullPath($LyFlowRuntime) } elseif ($env:GLUESIGHT_LYFLOW_RUNTIME) { $env:GLUESIGHT_LYFLOW_RUNTIME } else {
    $replayRestored = & pwsh -NoProfile -File (Join-Path $PSScriptRoot 'Restore-LyFlowRuntime.ps1')
    if ($LASTEXITCODE -eq 0) { @($replayRestored)[-1] }
}
$replayMissing = @()
if (!$replaySampleSet -or !(Test-Path -LiteralPath (Join-Path $replaySampleSet 'sample-set.json') -PathType Leaf)) { $replayMissing += "样本集目录缺少 sample-set.json：$replaySampleSet（-SampleSet 或 GLUESIGHT_SAMPLE_SET）" }
if (!$replayLyflow -or !(Test-Path -LiteralPath (Join-Path $replayLyflow 'lyflow_core.dll') -PathType Leaf)) { $replayMissing += "lyFlow 运行库目录缺少 lyflow_core.dll：$replayLyflow（-LyFlowRuntime 或 GLUESIGHT_LYFLOW_RUNTIME）" }
if ($replayMissing.Count) {
    [Console]::Error.WriteLine("离线回放验证输入缺失，未运行（不算通过，退出码 $replayMissingInputExit）：`n$($replayMissing -join "`n")")
    exit $replayMissingInputExit
}

$replayProjectRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$replayInstanceOutput = Join-Path $replayProjectRoot 'output\playwright\e2e-regression'
$replayManifestPath = Join-Path $replayInstanceOutput 'instance.json'
$replayExecutable = Join-Path $replayInstanceOutput 'gluesight-ui-tests.exe'
$replayOutput = Join-Path $replayProjectRoot 'output\playwright\log-replay'
$replayPreviousLyflow = $env:GLUESIGHT_LYFLOW_RUNTIME
$replayOwned = $null
$replayExit = 0
$replayWatch = [Diagnostics.Stopwatch]::StartNew()
Push-Location -LiteralPath $replayProjectRoot
try {
    & node --input-type=module -e "await import('playwright-core');"
    if ($LASTEXITCODE -ne 0) { throw '请使用 Node 24.15+ 并先执行 pnpm install --frozen-lockfile。' }
    $env:GLUESIGHT_LYFLOW_RUNTIME = $replayLyflow
    & (Join-Path $PSScriptRoot 'Start-UiTestInstance.ps1') -Profile e2e -ResetData -DebugPort $DebugPort -SkipBuild:$SkipBuild -DataRoot $DataRoot | Out-Host
    $replayOwned = Get-Content -LiteralPath $replayManifestPath -Raw | ConvertFrom-Json
    & node (Join-Path $replayProjectRoot 'tests\native\log-replay-offline.mjs') `
        --instance $replayManifestPath --sample-set $replaySampleSet --output $replayOutput --parts $Parts
    $replayExit = $LASTEXITCODE
    if ($replayExit -ne 0) { Write-Error "离线回放验证失败（退出码 $replayExit）。日志见 $replayOutput 与 $replayInstanceOutput。" -ErrorAction Continue }
} finally {
    $env:GLUESIGHT_LYFLOW_RUNTIME = $replayPreviousLyflow
    if ($replayOwned) {
        $replayProcess = Get-Process -Id $replayOwned.pid -ErrorAction SilentlyContinue
        if ($replayProcess) {
            if ($replayOwned.identifier -eq 'com.xyzrobotics.tujiaovision.e2e-tests' -and $replayOwned.executable -eq $replayExecutable -and
                $replayProcess.Path -eq $replayExecutable -and $replayProcess.StartTime.ToUniversalTime() -eq ([datetime]$replayOwned.startedAt).ToUniversalTime()) {
                Stop-Process -InputObject $replayProcess
                $replayProcess.WaitForExit(15000) | Out-Null
            } else { Write-Warning '测试进程身份已改变，未停止该进程。' }
        }
    }
    Pop-Location
    Write-Host "离线回放验证总耗时 $([math]::Round($replayWatch.Elapsed.TotalSeconds, 1)) s"
}
exit $replayExit
