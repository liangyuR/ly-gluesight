#Requires -Version 7.0
param(
    [ValidateRange(1024, 65535)][int]$DebugPort = 9341,
    [switch]$SkipBuild
)

$ErrorActionPreference = 'Stop'
$e2eMissingInputExit = 3
if (!$IsWindows) { throw '离线端到端测试需要 Windows、WebView2 与 lyFlow。' }
$e2eSampleSet = if ($env:GLUESIGHT_SAMPLE_SET) { $env:GLUESIGHT_SAMPLE_SET } else { 'C:\Users\11601\gluesight-samples\mx11-demo' }
$e2eLyflow = if ($env:LYFLOW_CORE_DLL) { $env:LYFLOW_CORE_DLL } else { 'C:\Users\11601\project\ly-gluesight\src-tauri\target\dependency-runtime\runtime\lyflow\lyflow_core.dll' }
$e2eMissing = @()
if (!(Test-Path -LiteralPath (Join-Path $e2eSampleSet 'sample-set.json') -PathType Leaf)) { $e2eMissing += "样本集目录缺少 sample-set.json：$e2eSampleSet（GLUESIGHT_SAMPLE_SET）" }
if (!(Test-Path -LiteralPath $e2eLyflow -PathType Leaf)) { $e2eMissing += "lyFlow 核心库不存在：$e2eLyflow（LYFLOW_CORE_DLL）" }
if ($e2eMissing.Count) {
    [Console]::Error.WriteLine("离线端到端输入缺失，未运行（不算通过，退出码 $e2eMissingInputExit）：`n$($e2eMissing -join "`n")")
    exit $e2eMissingInputExit
}

$e2eProjectRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$e2eOutput = Join-Path $e2eProjectRoot 'output\playwright\e2e-regression'
$e2eManifestPath = Join-Path $e2eOutput 'instance.json'
$e2eIdentifier = 'com.xyzrobotics.tujiaovision.e2e-tests'
$e2eOwnedInstance = $null
$e2eExit = 0
$e2eWatch = [Diagnostics.Stopwatch]::StartNew()
$e2ePreviousLyflow = $env:LYFLOW_CORE_DLL
Push-Location -LiteralPath $e2eProjectRoot
try {
    & node --input-type=module -e "await import('playwright-core');"
    if ($LASTEXITCODE -ne 0) { throw '请使用 Node 24.15+ 并先执行 pnpm install --frozen-lockfile。' }
    $env:LYFLOW_CORE_DLL = $e2eLyflow
    & (Join-Path $PSScriptRoot 'Start-UiTestInstance.ps1') -Profile e2e -ResetData -DebugPort $DebugPort -SkipBuild:$SkipBuild | Out-Host
    $e2eOwnedInstance = Get-Content -LiteralPath $e2eManifestPath -Raw | ConvertFrom-Json
    & node (Join-Path $e2eProjectRoot 'tests\native\offline-e2e.mjs') `
        --cdp "http://127.0.0.1:$DebugPort" --instance $e2eManifestPath --output (Join-Path $e2eOutput 'result.json') `
        --sample-set $e2eSampleSet --lyflow $e2eLyflow
    $e2eExit = $LASTEXITCODE
    if ($e2eExit -ne 0 -and $e2eExit -ne $e2eMissingInputExit) { Write-Error "离线端到端测试失败（退出码 $e2eExit）。日志见 $e2eOutput。" -ErrorAction Continue }
} finally {
    $env:LYFLOW_CORE_DLL = $e2ePreviousLyflow
    if ($e2eOwnedInstance) {
        $e2eOwnedProcess = Get-Process -Id $e2eOwnedInstance.pid -ErrorAction SilentlyContinue
        if ($e2eOwnedProcess) {
            $e2eExpectedExecutable = Join-Path $e2eOutput 'gluesight-ui-tests.exe'
            if ($e2eOwnedInstance.identifier -eq $e2eIdentifier -and
                $e2eOwnedInstance.executable -eq $e2eExpectedExecutable -and $e2eOwnedProcess.Path -eq $e2eExpectedExecutable -and
                $e2eOwnedProcess.StartTime.ToUniversalTime() -eq ([datetime]$e2eOwnedInstance.startedAt).ToUniversalTime()) {
                Stop-Process -InputObject $e2eOwnedProcess
            } else { Write-Warning '测试进程身份已改变，未停止该进程。' }
        }
    }
    Pop-Location
    Write-Host "离线端到端总耗时 $([math]::Round($e2eWatch.Elapsed.TotalSeconds, 1)) s"
}
exit $e2eExit
