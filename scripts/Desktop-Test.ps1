#Requires -Version 7.0
. (Join-Path $PSScriptRoot 'UiTest-Common.ps1')

function Invoke-DesktopTests {
    param(
        [int]$DebugPort,
        [string]$DataRoot,
        [switch]$SkipBuild,
        [switch]$ResetData,
        [string]$LyFlowRuntime,
        [ValidateSet('legacy', 'burst', 'all')][string]$ProductionSuite = 'all',
        [string]$Grep
    )
    $layout = Get-UiTestLayout -Profile flow
    $lock = Enter-UiTestLock -Path (Join-Path $layout.Output '.run.lock')
    $watch = [Diagnostics.Stopwatch]::StartNew()
    $savedOptions = $env:GLUESIGHT_DESKTOP_RUN
    $savedRuntime = $env:GLUESIGHT_LYFLOW_RUNTIME
    $runId = [guid]::NewGuid().ToString('N')
    $result = 1
    $resultPath = Join-Path $layout.Output 'result.json'
    Push-Location -LiteralPath $layout.Root
    try {
        if ($ProductionSuite -eq 'legacy') { $LyFlowRuntime = '' }
        $env:GLUESIGHT_LYFLOW_RUNTIME = $LyFlowRuntime
        $options = [ordered]@{
            suite = 'production'; profile = 'flow'; debugPort = $DebugPort; dataRoot = $DataRoot
            lyflow = $LyFlowRuntime; productionSuite = $ProductionSuite; resetData = [bool]$ResetData
            output = $layout.Output; manifest = $layout.Manifest; runId = $runId
        }
        $optionsPath = Join-Path $layout.Output 'run-options.json'
        $options | ConvertTo-Json | Set-Content -LiteralPath $optionsPath -Encoding utf8NoBOM
        $env:GLUESIGHT_DESKTOP_RUN = $optionsPath
        $initial = @{ passed = $false }
        if ($ProductionSuite -eq 'legacy') { $initial.burst = @{ skipped = '仅选择旧配方，段配方未运行。' } }
        elseif (!$LyFlowRuntime) { $initial.burst = @{ skipped = '未提供 lyFlow 运行库，段配方未运行。' } }
        $initial | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath $resultPath -Encoding utf8NoBOM
        & node --input-type=module -e "await import('@playwright/test'); await import('node:sqlite');"
        if ($LASTEXITCODE -ne 0) { throw '请先执行 pnpm install --frozen-lockfile 并使用项目要求的 Node 版本。' }
        if ($SkipBuild) { Assert-UiTestBuild -Layout $layout } else {
            & pwsh -NoProfile -File (Join-Path $PSScriptRoot 'Build-UiTestInstance.ps1') -Profile flow -RunId $runId 2>&1 |
                Tee-Object -FilePath (Join-Path $layout.Output 'build.log') | Out-Host
            if ($LASTEXITCODE -ne 0) { throw "桌面测试构建失败，日志见 $($layout.Output)/build.log。" }
        }
        $runnerArgs = @('node_modules/@playwright/test/cli.js', 'test', '--config', 'playwright.desktop.config.mjs', '--project', 'production')
        if ($Grep) { $runnerArgs += @('--grep', $Grep) }
        & node @runnerArgs | Out-Host
        $result = $LASTEXITCODE
        if ($result -ne 0) { Write-Warning "桌面测试失败（退出码 $result），报告见 $($layout.Output)/runner。" }
    } catch {
        Write-Error $_ -ErrorAction Continue
        $result = 1
    } finally {
        try {
            if (Test-Path -LiteralPath $layout.Manifest -PathType Leaf) {
                $instance = Get-Content -LiteralPath $layout.Manifest -Raw | ConvertFrom-Json
                if ($instance.runId -eq $runId) {
                    & pwsh -NoProfile -File (Join-Path $PSScriptRoot 'Stop-UiTestInstance.ps1') -Profile flow -Instance ($instance | ConvertTo-Json -Compress) | Out-Host
                    if ($LASTEXITCODE -ne 0) { throw '隔离实例最终清理失败。' }
                }
            }
        } catch { Write-Warning "测试结束后的实例清理失败：$_"; $result = 1 }
        if ($result -ne 0 -and (Test-Path -LiteralPath $resultPath -PathType Leaf)) {
            try {
                $summary = Get-Content -LiteralPath $resultPath -Raw | ConvertFrom-Json -AsHashtable
                $summary.passed = $false
                $summary.runnerExitCode = $result
                $summary | ConvertTo-Json -Depth 100 | Set-Content -LiteralPath $resultPath -Encoding utf8NoBOM
            } catch { Write-Warning "更新最终失败结果失败：$_" }
        }
        $env:GLUESIGHT_DESKTOP_RUN = $savedOptions
        $env:GLUESIGHT_LYFLOW_RUNTIME = $savedRuntime
        Pop-Location
        $lock.Dispose()
        Write-Host "桌面测试总耗时 $([math]::Round($watch.Elapsed.TotalSeconds, 1)) s"
    }
    return [int]$result
}
