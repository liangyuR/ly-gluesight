#Requires -Version 7.0
# 端到端外壳共用（Test-OfflineE2E、Test-LogReplayOffline、Test-Soak dot-source 本文件）：
# 输入检查与 lyFlow 运行库解析、Node 依赖检查、隔离实例启动，以及按 PID、标识、可执行文件路径与启动时间核对身份后才结束进程。
# 状态放在调用方脚本作用域的 $script:E2ERun 里（New-E2ERun 创建）。

$script:E2EMissingInputExit = 3

# 样本集与 lyFlow 运行库：参数优先，其次环境变量；运行库都没有时按 packaging/runtime.json 恢复（-NoRestore 不恢复）。
# 任一缺失或不存在时打印缺失项并以退出码 3 结束（不算通过），不启动实例。
function Resolve-E2EInputs {
    param([string]$Title, [string]$SampleSet, [string]$LyFlowRuntime, [switch]$NoRestore)
    $sampleSetDir = if ($SampleSet) { $SampleSet } else { $env:GLUESIGHT_SAMPLE_SET }
    $lyflow = if ($LyFlowRuntime) { [IO.Path]::GetFullPath($LyFlowRuntime) } elseif ($env:GLUESIGHT_LYFLOW_RUNTIME) { $env:GLUESIGHT_LYFLOW_RUNTIME } elseif (!$NoRestore) {
        $restored = & pwsh -NoProfile -File (Join-Path $PSScriptRoot 'Restore-LyFlowRuntime.ps1')
        if ($LASTEXITCODE -eq 0) { @($restored)[-1] }
    }
    $missing = @()
    if (!$sampleSetDir) { $missing += '未提供样本集目录（-SampleSet 或 GLUESIGHT_SAMPLE_SET）' }
    elseif (!(Test-Path -LiteralPath (Join-Path $sampleSetDir 'sample-set.json') -PathType Leaf)) { $missing += "样本集目录缺少 sample-set.json：$sampleSetDir（-SampleSet 或 GLUESIGHT_SAMPLE_SET）" }
    if (!$lyflow -and $NoRestore) { $missing += '未提供 lyFlow 运行库目录（-LyFlowRuntime 或 GLUESIGHT_LYFLOW_RUNTIME）' }
    elseif (!$lyflow) { $missing += 'lyFlow 运行库恢复失败且未提供（-LyFlowRuntime 或 GLUESIGHT_LYFLOW_RUNTIME）' }
    elseif (!(Test-Path -LiteralPath (Join-Path $lyflow 'lyflow_core.dll') -PathType Leaf)) { $missing += "lyFlow 运行库目录缺少 lyflow_core.dll：$lyflow" }
    if ($missing.Count) {
        [Console]::Error.WriteLine("${Title}输入缺失，未运行（不算通过，退出码 $script:E2EMissingInputExit）：`n$($missing -join "`n")")
        exit $script:E2EMissingInputExit
    }
    [pscustomobject]@{ SampleSet = $sampleSetDir; LyFlow = $lyflow }
}

function Assert-E2ENodeDependencies {
    & node --input-type=module -e "await import('playwright-core');"
    if ($LASTEXITCODE -ne 0) { throw '请使用 Node 24.15+ 并先执行 pnpm install --frozen-lockfile。' }
}

# 一次运行的隔离实例约定，与 Start-UiTestInstance.ps1 一致：输出目录 output\playwright\<Profile>-regression，
# 清单 instance.json，可执行文件 gluesight-ui-tests.exe，应用标识 com.xyzrobotics.tujiaovision.<Profile>-tests。
# 同时记下调用前的 GLUESIGHT_LYFLOW_RUNTIME，供 Restore-E2ELyFlowRuntime 恢复。
function New-E2ERun {
    param([string]$InstanceProfile, [int]$DebugPort, [string]$DataRoot, [string]$WarningPrefix = '')
    $root = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
    $output = Join-Path $root "output\playwright\$InstanceProfile-regression"
    $script:E2ERun = [pscustomobject]@{
        Root = $root; Profile = $InstanceProfile; DebugPort = $DebugPort; DataRoot = $DataRoot; WarningPrefix = $WarningPrefix
        Output = $output; Manifest = Join-Path $output 'instance.json'; Executable = Join-Path $output 'gluesight-ui-tests.exe'
        Identifier = "com.xyzrobotics.tujiaovision.$InstanceProfile-tests"
        PreviousLyFlow = $env:GLUESIGHT_LYFLOW_RUNTIME; Owned = $null; Watch = [Diagnostics.Stopwatch]::StartNew()
    }
    $script:E2ERun
}

function Restore-E2ELyFlowRuntime { $env:GLUESIGHT_LYFLOW_RUNTIME = $script:E2ERun.PreviousLyFlow }

function Start-E2EInstance([switch]$Fresh, [switch]$Prebuilt) {
    $run = $script:E2ERun
    & (Join-Path $PSScriptRoot 'Start-UiTestInstance.ps1') -Profile $run.Profile -ResetData:$Fresh -DebugPort $run.DebugPort -SkipBuild:$Prebuilt -DataRoot $run.DataRoot | Out-Host
    $run.Owned = Get-Content -LiteralPath $run.Manifest -Raw | ConvertFrom-Json
}

# 只结束本次启动、身份仍一致的进程；不一致时警告并保留进程。-Kill 强制结束（长稳的事务中断重启）。
function Stop-E2EInstance([switch]$Kill) {
    $run = $script:E2ERun
    $owned = $run.Owned
    $run.Owned = $null
    if (!$owned) { return }
    $process = Get-Process -Id $owned.pid -ErrorAction SilentlyContinue
    if (!$process) { return }
    if ($owned.identifier -ne $run.Identifier -or $owned.executable -ne $run.Executable -or $process.Path -ne $run.Executable -or
        $process.StartTime.ToUniversalTime() -ne ([datetime]$owned.startedAt).ToUniversalTime()) {
        Write-Warning "$($run.WarningPrefix)测试进程身份已改变，未停止该进程。"
        return
    }
    if ($Kill) { Stop-Process -InputObject $process -Force } else { Stop-Process -InputObject $process }
    $process.WaitForExit(15000) | Out-Null
}
