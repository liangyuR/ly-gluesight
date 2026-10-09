<#
.SYNOPSIS
在已验证的隔离桌面构建中运行海康样本离线回归。
.DESCRIPTION
先用 Start-UiTestInstance.ps1 构建并生成 instance.json，再结束该隔离测试实例。
本脚本复用清单中的程序与哈希，不读取生产应用配置。标准 UI 测试构建默认隐藏窗口。
.PARAMETER Archive
待测样本 ZIP 的完整路径，必须显式提供。
.PARAMETER InstanceManifest
隔离实例清单；默认 output/playwright/ui-regression/instance.json。
.PARAMETER DatasetRoot
转换后数据与报告目录；默认仓库 output/offline/hikvision-three-camera。
.PARAMETER ValidateOnly
只检查输入文件、隔离标识与程序哈希，不准备数据、启动程序或连接服务。
.EXAMPLE
pwsh -File scripts/Run-OfflineSampleTest.ps1 -Archive "E:\samples\three-camera.zip" -ValidateOnly
.EXAMPLE
pwsh -File scripts/Run-OfflineSampleTest.ps1 -Archive "E:\samples\three-camera.zip" -Headless
#>
param(
    [string]$Archive,
    [string]$DatasetRoot,
    [string]$InstanceManifest,
    [ValidateRange(1024, 65535)][int]$DebugPort = 9229,
    [switch]$PrepareOnly,
    [switch]$ValidateOnly,
    [switch]$Headless,
    [switch]$KeepOpen
)

$ErrorActionPreference = 'Stop'
$taskRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$taskDataset = if ($DatasetRoot) { [IO.Path]::GetFullPath($DatasetRoot) } else { Join-Path $taskRoot 'output\offline\hikvision-three-camera' }
if ([string]::IsNullOrWhiteSpace($Archive)) { throw '请用 -Archive 显式指定待测样本 ZIP 路径。' }
if (!(Test-Path -LiteralPath $Archive -PathType Leaf)) { throw 'Sample archive does not exist' }
$Archive = (Resolve-Path -LiteralPath $Archive).Path
$taskRunner = $null
$taskExpectedHash = $null
$taskNpx = $null
if (!$PrepareOnly) {
    $taskInstancePath = if ($InstanceManifest) { [IO.Path]::GetFullPath($InstanceManifest) } else { Join-Path $taskRoot 'output\playwright\ui-regression\instance.json' }
    if (!(Test-Path -LiteralPath $taskInstancePath -PathType Leaf)) {
        throw '缺少隔离实例清单。先运行 scripts/Start-UiTestInstance.ps1 生成清单并结束该实例，或用 -InstanceManifest 指定清单。'
    }
    $taskInstance = Get-Content -LiteralPath $taskInstancePath -Raw | ConvertFrom-Json
    if ($taskInstance.identifier -ne 'com.xyzrobotics.tujiaovision.ui-tests' -or
        [string]::IsNullOrWhiteSpace($taskInstance.executable) -or $taskInstance.sha256 -notmatch '^[A-Fa-f0-9]{64}$') {
        throw '隔离实例清单的应用标识、程序路径或 SHA256 无效。'
    }
    $taskRunner = if ([IO.Path]::IsPathFullyQualified($taskInstance.executable)) {
        [IO.Path]::GetFullPath($taskInstance.executable)
    } else {
        [IO.Path]::GetFullPath((Join-Path (Split-Path -Parent $taskInstancePath) $taskInstance.executable))
    }
    if (!(Test-Path -LiteralPath $taskRunner -PathType Leaf)) { throw '隔离测试程序不存在，请重新运行 Start-UiTestInstance.ps1 构建。' }
    $taskExpectedHash = $taskInstance.sha256
    if ((Get-FileHash -LiteralPath $taskRunner -Algorithm SHA256).Hash -ne $taskExpectedHash) {
        throw '隔离测试程序与清单 SHA256 不匹配，请重新构建；不要手动放宽哈希检查。'
    }
    $taskNpx = (Get-Command npx -CommandType Application -ErrorAction Stop | Select-Object -First 1).Source
}
if ($ValidateOnly) {
    @{ archive=$Archive; dataset=$taskDataset; executable=$taskRunner; sha256=$taskExpectedHash; prepareOnly=[bool]$PrepareOnly } |
        ConvertTo-Json -Compress
    return
}
$taskSession = 'tujiao-offline-repeat'
$taskProcess = $null
$taskStartedAt = $null
$taskSetupStarted = $false
$taskRestored = $false
$taskOldBrowserArguments = $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS

function Invoke-OfflineCode([string]$Script, [string]$Log) {
    $taskScriptPath = Join-Path $PSScriptRoot $Script
    if ($Script -eq 'offline-native-setup.js') {
        $taskSetupCode = Get-Content -LiteralPath $taskScriptPath -Raw
        $taskOptions = @{ datasetRoot=$taskDataset.Replace('\','/') } | ConvertTo-Json -Compress
        $taskScriptPath = Join-Path $taskDataset 'runner\repeat-setup-code.js'
        ('async (page) => (' + $taskSetupCode + ')(page, ' + $taskOptions + ')') |
            Set-Content -LiteralPath $taskScriptPath -Encoding utf8
    }
    $taskCliOutput = & $taskNpx --offline --package '@playwright/cli' playwright-cli "-s=$taskSession" run-code --filename $taskScriptPath
    $taskCliExit = $LASTEXITCODE
    $taskCliOutput | Set-Content -LiteralPath (Join-Path $taskDataset "runner\$Log") -Encoding utf8
    if ($taskCliExit -ne 0 -or ($taskCliOutput -match '^### Error')) {
        throw "Offline command failed: $Script. Inspect runner\$Log."
    }
    $taskResultLine = $taskCliOutput | Where-Object { $_.StartsWith('{') } | Select-Object -First 1
    if ($null -eq $taskResultLine) { throw "No result returned by $Script" }
    return $taskResultLine | ConvertFrom-Json
}

Push-Location -LiteralPath $taskRoot
try {
    $taskManifest = Join-Path $taskDataset 'dataset.json'
    if (!(Test-Path -LiteralPath $taskManifest -PathType Leaf)) {
        & python (Join-Path $PSScriptRoot 'prepare-offline-data.py') --archive $Archive --destination $taskDataset
    } else {
        $taskExisting = Get-Content -LiteralPath $taskManifest -Raw | ConvertFrom-Json
        if ((Get-FileHash -LiteralPath $Archive -Algorithm SHA256).Hash -ne $taskExisting.archiveSha256) {
            throw 'Archive changed. Prepare it in a separate output directory before reusing this dataset.'
        }
        & python (Join-Path $PSScriptRoot 'prepare-offline-data.py') --archive $Archive --destination $taskDataset --timed-only
    }
    if ($LASTEXITCODE -ne 0) { throw 'Dataset preparation failed' }
    if ($PrepareOnly) { Write-Output "Prepared replay data: $taskDataset"; return }
    New-Item -ItemType Directory -Path (Join-Path $taskDataset 'runner') -Force | Out-Null
    if ((Get-FileHash -LiteralPath $taskRunner -Algorithm SHA256).Hash -ne $taskExpectedHash) {
        throw 'Unexpected executable. Only the verified isolated test build may be launched by this script.'
    }
    $taskRunnerPaths = @(
        $taskRunner,
        (Join-Path $taskDataset 'runner\tujiao-offline-test.exe'),
        (Join-Path $taskDataset 'runner\tujiao-offline-visible.exe')
    )
    $taskExistingRunners = @(Get-Process -ErrorAction SilentlyContinue | Where-Object { $_.Path -in $taskRunnerPaths })
    if ($taskExistingRunners.Count) {
        $taskExistingIds = ($taskExistingRunners.Id -join ', ')
        throw "隔离测试实例仍在运行（PID $taskExistingIds）。请先关闭测试窗口，再重新运行；避免两个实例同时改写测试配置。"
    }
    $taskProbe = [Net.Sockets.TcpListener]::new([Net.IPAddress]::Loopback, $DebugPort)
    try { $taskProbe.Start() } finally { $taskProbe.Stop() }
    $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = "--remote-debugging-port=$DebugPort --remote-debugging-address=127.0.0.1"
    $taskLaunch = @{
        FilePath = $taskRunner
        WorkingDirectory = $taskRoot
        WindowStyle = 'Hidden'
        PassThru = $true
        RedirectStandardOutput = Join-Path $taskDataset 'runner\repeat-stdout.txt'
        RedirectStandardError = Join-Path $taskDataset 'runner\repeat-stderr.txt'
    }
    $taskProcess = Start-Process @taskLaunch
    $taskStartedAt = $taskProcess.StartTime.ToUniversalTime().Ticks
    Write-Output '已启动已验证的隔离测试构建；窗口是否可见由该构建配置决定，标准 UI 测试构建默认隐藏。'
    $taskReady = $false
    for ($taskAttempt = 0; $taskAttempt -lt 40; $taskAttempt++) {
        try {
            $taskVersion = Invoke-RestMethod -Uri "http://127.0.0.1:$DebugPort/json/version" -TimeoutSec 1
            $taskReady = [bool]$taskVersion.webSocketDebuggerUrl
        } catch { }
        if ($taskReady) { break }
        if ($taskProcess.HasExited) { throw 'Isolated desktop runner exited before browser initialization' }
        Start-Sleep -Milliseconds 250
    }
    if (!$taskReady) { throw 'Isolated desktop WebView did not become ready' }
    & $taskNpx --offline --package '@playwright/cli' playwright-cli "-s=$taskSession" attach "--cdp=http://127.0.0.1:$DebugPort"
    if ($LASTEXITCODE -ne 0) { throw 'Could not attach to isolated WebView' }
    $null = Invoke-OfflineCode 'offline-native-state.js' 'repeat-initial-state.log'
    $taskSetupStarted = $true
    $null = Invoke-OfflineCode 'offline-native-setup.js' 'repeat-setup.log'
    foreach ($taskGroup in @(@{Name='Glue1';Batches=6}, @{Name='Glue2';Batches=5})) {
        for ($taskBatch = 1; $taskBatch -le $taskGroup.Batches; $taskBatch++) {
            $taskResult = Invoke-OfflineCode 'offline-native-batch.js' ("repeat-{0}-batch-{1}.log" -f $taskGroup.Name, $taskBatch)
            Write-Output ("{0}: frames {1}-{2} complete" -f $taskResult.group, $taskResult.first, $taskResult.last)
        }
        $taskCheck = Invoke-OfflineCode 'offline-native-next-group.js' ("repeat-{0}-summary.log" -f $taskGroup.Name)
        if (!$taskCheck.passed) { throw 'Replay continuity check failed' }
    }
    for ($taskCycle = 1; $taskCycle -le 4; $taskCycle++) {
        $taskResult = Invoke-OfflineCode 'offline-native-cycle.js' "repeat-cycle-$taskCycle.log"
        if (!$taskResult.passedPipeline) { throw 'Complete-cycle image pipeline failed' }
        Write-Output ("{0}: {1}, verdict {2}" -f $taskResult.name, $taskResult.experiment, $taskResult.summary.verdict)
    }
    $null = Invoke-OfflineCode 'offline-native-screenshots.js' 'repeat-screenshots.log'
    $taskRestore = Invoke-OfflineCode 'offline-native-restore.js' 'repeat-restoration.log'
    $taskRestored = $taskRestore.cameraConfigurationsRestored -and $taskRestore.cycleSettingsRestored -and $taskRestore.temporaryRecipesRemoved
    if (!$taskRestored) { throw 'Isolated configuration restoration did not pass' }
    $null = Invoke-OfflineCode 'offline-native-export.js' 'repeat-native-report.log'
    $taskJsonLine = Get-Content -LiteralPath (Join-Path $taskDataset 'runner\repeat-native-report.log') |
        Where-Object { $_.StartsWith('{"passedInfrastructure":') } | Select-Object -First 1
    if (!$taskJsonLine) { throw 'Native report export did not return JSON' }
    $taskReport = $taskJsonLine | ConvertFrom-Json
    $taskReport | Add-Member -NotePropertyName testedExecutableSha256 -NotePropertyValue $taskExpectedHash.ToLowerInvariant() -Force
    $taskReport | Add-Member -NotePropertyName testedExecutablePath -NotePropertyValue $taskRunner -Force
    $taskReport | ConvertTo-Json -Depth 100 -Compress | Set-Content -LiteralPath (Join-Path $taskDataset 'native-report.json') -Encoding utf8
    & python (Join-Path $PSScriptRoot 'summarize-offline-test.py') --root $taskDataset
    if ($LASTEXITCODE -ne 0) { throw 'Native result validation failed' }
    Write-Output "Offline test report: $(Join-Path $taskDataset 'REPORT.md')"
    if (!$Headless -and $KeepOpen) {
        $null = Invoke-OfflineCode 'offline-native-finish.js' 'repeat-visible-finish.log'
    }
} finally {
    if ($taskSetupStarted -and !$taskRestored) {
        try {
            $taskRestore = Invoke-OfflineCode 'offline-native-restore.js' 'repeat-restoration-after-error.log'
            Write-Output ("Restoration after error: cameras={0}, settings={1}" -f $taskRestore.cameraConfigurationsRestored, $taskRestore.cycleSettingsRestored)
        } catch { Write-Warning "Inspect the isolated test configuration before another run: $_" }
    }
    if ($null -ne $taskProcess) {
        $taskCurrent = Get-Process -Id $taskProcess.Id -ErrorAction SilentlyContinue
        if ($null -ne $taskCurrent -and $taskCurrent.Path -eq $taskRunner -and
            $taskCurrent.StartTime.ToUniversalTime().Ticks -eq $taskStartedAt -and
            (!$KeepOpen -or $Headless -or !$taskRestored)) {
            Stop-Process -InputObject $taskCurrent
            $null = $taskProcess.WaitForExit(5000)
        }
        if ($KeepOpen -and !$Headless -and $taskRestored) {
            Write-Output "测试完成，隔离进程保留在最后一件历史详情。实验配置已恢复；结束该实例后可再次运行。（PID $($taskProcess.Id)）"
        } else {
            & $taskNpx --offline --package '@playwright/cli' playwright-cli "-s=$taskSession" close
        }
    }
    $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = $taskOldBrowserArguments
    Pop-Location
}
