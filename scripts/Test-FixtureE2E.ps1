#Requires -Version 7.0
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
$e2eMissingInputExit = 3
if (!$IsWindows) { throw '特征定位端到端测试需要 Windows、WebView2 与 lyFlow。' }
$e2eSampleSet = if ($SampleSet) { $SampleSet } else { $env:GLUESIGHT_SAMPLE_SET }
$e2eLyflow = if ($LyFlowRuntime) { [IO.Path]::GetFullPath($LyFlowRuntime) } elseif ($env:GLUESIGHT_LYFLOW_RUNTIME) { $env:GLUESIGHT_LYFLOW_RUNTIME } else {
    $e2eRestored = & pwsh -NoProfile -File (Join-Path $PSScriptRoot 'Restore-LyFlowRuntime.ps1')
    if ($LASTEXITCODE -eq 0) { @($e2eRestored)[-1] }
}
$e2eMissing = @()
if (!$e2eSampleSet) { $e2eMissing += '未提供样本集目录（-SampleSet 或 GLUESIGHT_SAMPLE_SET）' }
elseif (!(Test-Path -LiteralPath (Join-Path $e2eSampleSet 'sample-set.json') -PathType Leaf)) { $e2eMissing += "样本集目录缺少 sample-set.json：$e2eSampleSet（-SampleSet 或 GLUESIGHT_SAMPLE_SET）" }
if (!$e2eLyflow) { $e2eMissing += 'lyFlow 运行库恢复失败且未提供（-LyFlowRuntime 或 GLUESIGHT_LYFLOW_RUNTIME）' }
elseif (!(Test-Path -LiteralPath (Join-Path $e2eLyflow 'lyflow_core.dll') -PathType Leaf)) { $e2eMissing += "lyFlow 运行库目录缺少 lyflow_core.dll：$e2eLyflow" }
if ($e2eMissing.Count) {
    [Console]::Error.WriteLine("特征定位端到端输入缺失，未运行（不算通过，退出码 $e2eMissingInputExit）：`n$($e2eMissing -join "`n")")
    exit $e2eMissingInputExit
}

$e2eProjectRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$e2eOutput = Join-Path $e2eProjectRoot 'output\playwright\e2e-regression'
$e2eManifestPath = Join-Path $e2eOutput 'instance.json'
$e2eIdentifier = 'com.xyzrobotics.tujiaovision.e2e-tests'
$e2eExpectedExecutable = Join-Path $e2eOutput 'gluesight-ui-tests.exe'
$e2eExit = 0
$e2eWatch = [Diagnostics.Stopwatch]::StartNew()
$e2ePreviousLyflow = $env:GLUESIGHT_LYFLOW_RUNTIME
$e2eOwned = $null

Push-Location -LiteralPath $e2eProjectRoot
try {
    & node --input-type=module -e "await import('playwright-core');"
    if ($LASTEXITCODE -ne 0) { throw '请使用 Node 24.15+ 并先执行 pnpm install --frozen-lockfile。' }
    # 只在本进程设运行库目录，不改用户级变量
    $env:GLUESIGHT_LYFLOW_RUNTIME = $e2eLyflow
    & (Join-Path $PSScriptRoot 'Start-UiTestInstance.ps1') -Profile e2e -ResetData -DebugPort $DebugPort -SkipBuild:$SkipBuild -DataRoot $DataRoot | Out-Host
    $e2eOwned = Get-Content -LiteralPath $e2eManifestPath -Raw | ConvertFrom-Json
    & node (Join-Path $e2eProjectRoot 'tests\native\fixture-teach-e2e.mjs') `
        --instance $e2eManifestPath --output (Join-Path $e2eOutput 'fixture\result.json') --sample-set $e2eSampleSet --lyflow-runtime $e2eLyflow
    $e2eExit = $LASTEXITCODE
    if ($e2eExit -ne 0 -and $e2eExit -ne $e2eMissingInputExit) { Write-Error "特征定位端到端测试失败（退出码 $e2eExit）。日志见 $e2eOutput。" -ErrorAction Continue }
} finally {
    $env:GLUESIGHT_LYFLOW_RUNTIME = $e2ePreviousLyflow
    if ($e2eOwned) {
        $process = Get-Process -Id $e2eOwned.pid -ErrorAction SilentlyContinue
        if ($process) {
            if ($e2eOwned.identifier -eq $e2eIdentifier -and $e2eOwned.executable -eq $e2eExpectedExecutable -and $process.Path -eq $e2eExpectedExecutable -and
                $process.StartTime.ToUniversalTime() -eq ([datetime]$e2eOwned.startedAt).ToUniversalTime()) {
                Stop-Process -InputObject $process
                $process.WaitForExit(15000) | Out-Null
            } else { Write-Warning '测试进程身份已改变，未停止该进程。' }
        }
    }
    Pop-Location
    Write-Host "特征定位端到端总耗时 $([math]::Round($e2eWatch.Elapsed.TotalSeconds, 1)) s"
}
exit $e2eExit
