#Requires -Version 7.0
param(
    [ValidateRange(1024, 65535)][int]$DebugPort = 9347,
    [switch]$SkipBuild,
    [string]$SampleSet,
    [string]$LyFlowRuntime,
    [ValidateRange(1, 1000000)][int]$Parts = 1000,
    [ValidateRange(50, 5000)][int]$ShotIntervalMs = 150,
    [ValidateRange(0, 10000)][int]$PauseMs = 200,
    [ValidateRange(0.0, 1.0)][double]$MaxErrRatio = 0.2,
    [string[]]$Stages = @('soak', 'faults', 'engine', 'audit', 'restart')
)

$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'E2E-Common.ps1')
if (!$IsWindows) { throw '长稳测试需要 Windows、WebView2 与 lyFlow。' }
$Stages = @($Stages | ForEach-Object { $_ -split ',' } | ForEach-Object { $_.Trim() } | Where-Object { $_ })
$soakUnknown = @($Stages | Where-Object { $_ -notin 'soak', 'faults', 'engine', 'disk', 'audit', 'restart' })
if ($soakUnknown.Count) { throw "未知阶段：$($soakUnknown -join ', ')；可选 soak、faults、engine、disk、audit、restart" }
# 长稳不自动恢复 lyFlow 运行库：参数与环境变量都没有时按输入缺失处理（退出码 3）。
$soakInputs = Resolve-E2EInputs -Title '长稳测试' -SampleSet $SampleSet -LyFlowRuntime $LyFlowRuntime -NoRestore
$soakLyflow = $soakInputs.LyFlow

$soakRun = New-E2ERun -InstanceProfile soak -DebugPort $DebugPort -WarningPrefix '长稳'
$soakOutput = Join-Path $soakRun.Root 'output\soak'
$soakCdp = "http://[::1]:$DebugPort"
$soakNodeArgs = @('--cdp', $soakCdp, '--instance', $soakRun.Manifest, '--out', $soakOutput)
$soakExit = 0
$script:soakKnown = [Collections.Generic.List[string]]::new()
$script:soakSkipped = [Collections.Generic.List[string]]::new()

function Invoke-SoakNode([string]$Script, [string[]]$Arguments) {
    & node (Join-Path $PSScriptRoot "soak\$Script") @Arguments
    if ($LASTEXITCODE -eq 2) {
        $script:soakKnown.Add("$Script $($Arguments[0])")
        Write-Warning "$Script $($Arguments[0]) 只出现已知问题，继续后续阶段，整次运行不算通过（退出码 2）。"
        return
    }
    if ($LASTEXITCODE -ne 0) { throw "$Script $($Arguments[0]) 失败（退出码 $LASTEXITCODE）" }
}

Push-Location -LiteralPath $soakRun.Root
try {
    New-Item -ItemType Directory -Force -Path $soakOutput | Out-Null
    $env:GLUESIGHT_LYFLOW_RUNTIME = $soakLyflow
    Start-E2EInstance -Fresh -Prebuilt:$SkipBuild
    Invoke-SoakNode 'soak.mjs' (@('setup') + $soakNodeArgs + @('--sample-set', $soakInputs.SampleSet))
    if ($Stages -contains 'soak') {
        Invoke-SoakNode 'soak.mjs' (@('run') + $soakNodeArgs + @('--parts', "$Parts", '--interval', "$ShotIntervalMs", '--pause', "$PauseMs",
            '--max-err-ratio', $MaxErrRatio.ToString([Globalization.CultureInfo]::InvariantCulture)))
    }
    if ($Stages -contains 'faults') {
        Invoke-SoakNode 'faults.mjs' (@('run') + $soakNodeArgs)
    }
    if ($Stages -contains 'engine') {
        Stop-E2EInstance
        $soakEmptyRuntime = Join-Path $soakOutput 'empty-lyflow-runtime'
        New-Item -ItemType Directory -Force -Path $soakEmptyRuntime | Out-Null
        $env:GLUESIGHT_LYFLOW_RUNTIME = $soakEmptyRuntime
        try { Start-E2EInstance -Prebuilt } finally { $env:GLUESIGHT_LYFLOW_RUNTIME = $soakLyflow }
        Invoke-SoakNode 'engine.mjs' (@('missing') + $soakNodeArgs)
        Stop-E2EInstance
        Start-E2EInstance -Prebuilt
        Invoke-SoakNode 'faults.mjs' (@('recover') + $soakNodeArgs + @('--file', 'engine-recovered.json'))
    }
    if ($Stages -contains 'disk') {
        $soakDiskGuard = Get-ChildItem -LiteralPath (Join-Path $soakRun.Root 'src-tauri\src') -Recurse -Filter '*.rs' |
            Select-String -SimpleMatch 'GLUESIGHT_DISK_MIN_FREE_MB' -List | Select-Object -First 1
        if (!$soakDiskGuard) {
            $script:soakSkipped.Add('disk：本构建没有资料盘守护（读取 GLUESIGHT_DISK_MIN_FREE_MB 的实现随 #111 合入）')
            Write-Warning 'disk 阶段未运行：本构建没有资料盘守护，需 #111 先合入。'
        } else {
            Stop-E2EInstance
            $env:GLUESIGHT_DISK_MIN_FREE_MB = '1073741824'
            try { Start-E2EInstance -Prebuilt } finally { Remove-Item -LiteralPath Env:\GLUESIGHT_DISK_MIN_FREE_MB -ErrorAction SilentlyContinue }
            Invoke-SoakNode 'disk.mjs' (@('blocked') + $soakNodeArgs)
            Stop-E2EInstance
            Start-E2EInstance -Prebuilt
            Invoke-SoakNode 'disk.mjs' (@('recovered') + $soakNodeArgs)
        }
    }
    if ($Stages -contains 'audit') {
        Invoke-SoakNode 'faults.mjs' (@('audit') + $soakNodeArgs)
        Stop-E2EInstance
        Start-E2EInstance -Prebuilt
        Invoke-SoakNode 'faults.mjs' (@('recover') + $soakNodeArgs)
    }
    if ($Stages -contains 'restart') {
        Invoke-SoakNode 'restart.mjs' (@('arm') + $soakNodeArgs)
        Stop-E2EInstance -Kill
        Start-Sleep -Seconds 2
        Start-E2EInstance -Prebuilt
        Invoke-SoakNode 'restart.mjs' (@('verify') + $soakNodeArgs)
    }
} catch {
    $soakExit = 1
    Write-Error "长稳测试失败：$_。报告与日志见 $soakOutput 与 $($soakRun.Output)。" -ErrorAction Continue
} finally {
    Restore-E2ELyFlowRuntime
    Stop-E2EInstance
    Pop-Location
}
if ($script:soakSkipped.Count) { Write-Warning "未运行的阶段（不算通过也不算失败）：$($script:soakSkipped -join '；')" }
if ($soakExit -eq 0 -and $script:soakKnown.Count) {
    $soakExit = 2
    Write-Warning "仅出现已知问题，未全部通过（退出码 2）：$($script:soakKnown -join '、')。明细见 $soakOutput 中各结果文件的 knownIssues。"
}
exit $soakExit
