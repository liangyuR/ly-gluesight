#Requires -Version 7.0
param(
    [ValidateRange(1024, 65535)][int]$DebugPort = 9347,
    [switch]$SkipBuild,
    [string]$SampleSet,
    [string]$LyFlowRuntime,
    [ValidateRange(1, 1000000)][int]$Parts = 1000,
    [ValidateRange(50, 5000)][int]$ShotIntervalMs = 150,
    [ValidateRange(0, 10000)][int]$PauseMs = 200,
    [string[]]$Stages = @('soak', 'faults', 'engine', 'audit', 'restart')
)

$ErrorActionPreference = 'Stop'
if (!$IsWindows) { throw '长稳测试需要 Windows、WebView2 与 lyFlow。' }
$Stages = @($Stages | ForEach-Object { $_ -split ',' } | ForEach-Object { $_.Trim() } | Where-Object { $_ })
$soakUnknown = @($Stages | Where-Object { $_ -notin 'soak', 'faults', 'engine', 'disk', 'audit', 'restart' })
if ($soakUnknown.Count) { throw "未知阶段：$($soakUnknown -join ', ')；可选 soak、faults、engine、disk、audit、restart" }
$soakSampleSet = if ($SampleSet) { $SampleSet } else { $env:GLUESIGHT_SAMPLE_SET }
$soakLyflow = if ($LyFlowRuntime) { [IO.Path]::GetFullPath($LyFlowRuntime) } else { $env:GLUESIGHT_LYFLOW_RUNTIME }
$soakMissing = @()
if (!$soakSampleSet -or !(Test-Path -LiteralPath (Join-Path $soakSampleSet 'sample-set.json') -PathType Leaf)) { $soakMissing += "样本集目录缺少 sample-set.json：$soakSampleSet（-SampleSet 或 GLUESIGHT_SAMPLE_SET）" }
if (!$soakLyflow -or !(Test-Path -LiteralPath (Join-Path $soakLyflow 'lyflow_core.dll') -PathType Leaf)) { $soakMissing += "lyFlow 运行库目录缺少 lyflow_core.dll：$soakLyflow（-LyFlowRuntime 或 GLUESIGHT_LYFLOW_RUNTIME）" }
if ($soakMissing.Count) {
    [Console]::Error.WriteLine("长稳测试输入缺失，未运行（不算通过，退出码 3）：`n$($soakMissing -join "`n")")
    exit 3
}

$soakRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$soakInstanceDir = Join-Path $soakRoot 'output\playwright\soak-regression'
$soakManifest = Join-Path $soakInstanceDir 'instance.json'
$soakExecutable = Join-Path $soakInstanceDir 'gluesight-ui-tests.exe'
$soakOutput = Join-Path $soakRoot 'output\soak'
$soakCdp = "http://[::1]:$DebugPort"
$soakNodeArgs = @('--cdp', $soakCdp, '--instance', $soakManifest, '--out', $soakOutput)
$soakPreviousLyflow = $env:GLUESIGHT_LYFLOW_RUNTIME
$soakExit = 0
$script:soakOwned = $null

function Start-SoakInstance([switch]$Fresh, [switch]$Prebuilt) {
    & (Join-Path $PSScriptRoot 'Start-UiTestInstance.ps1') -Profile soak -ResetData:$Fresh -DebugPort $DebugPort -SkipBuild:$Prebuilt | Out-Host
    $script:soakOwned = Get-Content -LiteralPath $soakManifest -Raw | ConvertFrom-Json
}

function Stop-SoakInstance([switch]$Kill) {
    $owned = $script:soakOwned
    $script:soakOwned = $null
    if (!$owned) { return }
    $process = Get-Process -Id $owned.pid -ErrorAction SilentlyContinue
    if (!$process) { return }
    if ($owned.executable -ne $soakExecutable -or $process.Path -ne $soakExecutable -or
        $process.StartTime.ToUniversalTime() -ne ([datetime]$owned.startedAt).ToUniversalTime()) {
        Write-Warning '长稳测试进程身份已改变，未停止该进程。'
        return
    }
    if ($Kill) { Stop-Process -InputObject $process -Force } else { Stop-Process -InputObject $process }
    $process.WaitForExit(15000) | Out-Null
}

function Invoke-SoakNode([string]$Script, [string[]]$Arguments) {
    & node (Join-Path $PSScriptRoot "soak\$Script") @Arguments
    if ($LASTEXITCODE -ne 0) { throw "$Script $($Arguments[0]) 失败（退出码 $LASTEXITCODE）" }
}

Push-Location -LiteralPath $soakRoot
try {
    New-Item -ItemType Directory -Force -Path $soakOutput | Out-Null
    $env:GLUESIGHT_LYFLOW_RUNTIME = $soakLyflow
    Start-SoakInstance -Fresh -Prebuilt:$SkipBuild
    Invoke-SoakNode 'soak.mjs' (@('setup') + $soakNodeArgs + @('--sample-set', $soakSampleSet))
    if ($Stages -contains 'soak') {
        Invoke-SoakNode 'soak.mjs' (@('run') + $soakNodeArgs + @('--parts', "$Parts", '--interval', "$ShotIntervalMs", '--pause', "$PauseMs"))
    }
    if ($Stages -contains 'faults') {
        Invoke-SoakNode 'faults.mjs' (@('run') + $soakNodeArgs)
    }
    if ($Stages -contains 'engine') {
        Stop-SoakInstance
        $soakEmptyRuntime = Join-Path $soakOutput 'empty-lyflow-runtime'
        New-Item -ItemType Directory -Force -Path $soakEmptyRuntime | Out-Null
        $env:GLUESIGHT_LYFLOW_RUNTIME = $soakEmptyRuntime
        try { Start-SoakInstance -Prebuilt } finally { $env:GLUESIGHT_LYFLOW_RUNTIME = $soakLyflow }
        Invoke-SoakNode 'engine.mjs' (@('missing') + $soakNodeArgs)
        Stop-SoakInstance
        Start-SoakInstance -Prebuilt
        Invoke-SoakNode 'faults.mjs' (@('recover') + $soakNodeArgs + @('--file', 'engine-recovered.json'))
    }
    if ($Stages -contains 'disk') {
        Stop-SoakInstance
        $env:GLUESIGHT_DISK_MIN_FREE_MB = '1073741824'
        try { Start-SoakInstance -Prebuilt } finally { Remove-Item -LiteralPath Env:\GLUESIGHT_DISK_MIN_FREE_MB -ErrorAction SilentlyContinue }
        Invoke-SoakNode 'disk.mjs' (@('blocked') + $soakNodeArgs)
        Stop-SoakInstance
        Start-SoakInstance -Prebuilt
        Invoke-SoakNode 'disk.mjs' (@('recovered') + $soakNodeArgs)
    }
    if ($Stages -contains 'audit') {
        Invoke-SoakNode 'faults.mjs' (@('audit') + $soakNodeArgs)
        Stop-SoakInstance
        Start-SoakInstance -Prebuilt
        Invoke-SoakNode 'faults.mjs' (@('recover') + $soakNodeArgs)
    }
    if ($Stages -contains 'restart') {
        Invoke-SoakNode 'restart.mjs' (@('arm') + $soakNodeArgs)
        Stop-SoakInstance -Kill
        Start-Sleep -Seconds 2
        Start-SoakInstance -Prebuilt
        Invoke-SoakNode 'restart.mjs' (@('verify') + $soakNodeArgs)
    }
} catch {
    $soakExit = 1
    Write-Error "长稳测试失败：$_。报告与日志见 $soakOutput 与 $soakInstanceDir。" -ErrorAction Continue
} finally {
    $env:GLUESIGHT_LYFLOW_RUNTIME = $soakPreviousLyflow
    Stop-SoakInstance
    Pop-Location
}
exit $soakExit
