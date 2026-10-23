#Requires -Version 7.0
# 生产落盘流程（tests/native/production-flow-ui.mjs）：旧拍照点配方（首启样例，不需要 lyFlow）与段配方（合成图三相机）两段。
# 段配方一段要真实 lyFlow 测量合成图（走廊提取后的标定与验证）：-LyFlowRuntime 或环境变量 GLUESIGHT_LYFLOW_RUNTIME 提供运行库目录，
# 都没有时该段跳过（结果里写明 skipped），旧配方一段照常运行。
param(
    [ValidateRange(1024, 65535)][int]$DebugPort = 9339,
    [switch]$SkipBuild,
    [string]$DataRoot,
    [string]$LyFlowRuntime
)

$ErrorActionPreference = 'Stop'
if (!$IsWindows) { throw '桌面落盘测试需要 Windows 和 WebView2。' }
$flowProjectRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$flowOutput = Join-Path $flowProjectRoot 'output/playwright/flow-regression'
$flowManifestPath = Join-Path $flowOutput 'instance.json'
$flowOwnedInstance = $null
$flowWatch = [Diagnostics.Stopwatch]::StartNew()
$flowPreviousLyFlow = $env:GLUESIGHT_LYFLOW_RUNTIME
if ($LyFlowRuntime) {
    $flowLyFlow = [IO.Path]::GetFullPath($LyFlowRuntime)
    if (!(Test-Path -LiteralPath (Join-Path $flowLyFlow 'lyflow_core.dll') -PathType Leaf)) { throw "lyFlow 运行库目录缺少 lyflow_core.dll：$flowLyFlow" }
    $env:GLUESIGHT_LYFLOW_RUNTIME = $flowLyFlow
}
if (!$env:GLUESIGHT_LYFLOW_RUNTIME) { Write-Warning '未提供 lyFlow 运行库（-LyFlowRuntime 或 GLUESIGHT_LYFLOW_RUNTIME）：段配方一段将跳过，只验证旧拍照点配方。' }
Push-Location -LiteralPath $flowProjectRoot
try {
    & node --input-type=module -e "await import('playwright-core'); await import('node:sqlite');"
    if ($LASTEXITCODE -ne 0) { throw '请使用 Node 24.15+ 并先执行 pnpm install --frozen-lockfile。' }
    & (Join-Path $PSScriptRoot 'Start-UiTestInstance.ps1') -Profile flow -DebugPort $DebugPort -SkipBuild:$SkipBuild -DataRoot $DataRoot | Out-Host
    $flowOwnedInstance = Get-Content -LiteralPath $flowManifestPath -Raw | ConvertFrom-Json
    & node (Join-Path $flowProjectRoot 'tests/native/production-flow-ui.mjs') `
        --instance $flowManifestPath --output (Join-Path $flowOutput 'result.json')
    if ($LASTEXITCODE -ne 0) { throw "生产落盘测试失败（退出码 $LASTEXITCODE）。日志见 $flowOutput。" }
} finally {
    $env:GLUESIGHT_LYFLOW_RUNTIME = $flowPreviousLyFlow
    if ($flowOwnedInstance) {
        $flowOwnedProcess = Get-Process -Id $flowOwnedInstance.pid -ErrorAction SilentlyContinue
        if ($flowOwnedProcess) {
            $flowExpectedExecutable = Join-Path $flowOutput 'gluesight-ui-tests.exe'
            if ($flowOwnedInstance.identifier -eq 'com.xyzrobotics.tujiaovision.flow-tests' -and
                $flowOwnedInstance.executable -eq $flowExpectedExecutable -and $flowOwnedProcess.Path -eq $flowExpectedExecutable -and
                $flowOwnedProcess.StartTime.ToUniversalTime() -eq ([datetime]$flowOwnedInstance.startedAt).ToUniversalTime()) {
                Stop-Process -InputObject $flowOwnedProcess
            } else { Write-Warning '测试进程身份已改变，未停止该进程。' }
        }
    }
    Pop-Location
    Write-Host "生产落盘总耗时 $([math]::Round($flowWatch.Elapsed.TotalSeconds, 1)) s"
}
