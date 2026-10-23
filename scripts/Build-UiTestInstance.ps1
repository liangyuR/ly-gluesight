#Requires -Version 7.0
param(
    [ValidateSet('ui', 'p0', 'flow', 'e2e', 'soak')][string]$Profile = 'ui',
    [string]$RunId
)
$ErrorActionPreference = 'Stop'
if (!$IsWindows) { throw '隔离桌面构建需要 Windows。' }
. (Join-Path $PSScriptRoot 'UiTest-Common.ps1')
$layout = Get-UiTestLayout -Profile $Profile
$runLock = if (!$RunId) { Enter-UiTestLock -Path (Join-Path $layout.Output '.run.lock') }
$buildLock = $null
Push-Location -LiteralPath $layout.Root
try {
    $buildLock = Enter-UiTestLock -Path (Join-Path $layout.Target '.ui-test-build.lock')
    $existing = @(Get-Process -ErrorAction SilentlyContinue | Where-Object { $_.Path -eq $layout.Executable })
    if ($existing.Count) { throw "隔离 UI 实例仍在运行：$($existing.Id -join ', ')。" }
    [void][IO.Directory]::CreateDirectory($layout.Output)
    Remove-Item -LiteralPath $layout.BuildManifest -ErrorAction SilentlyContinue
    $layout.Overlay | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath $layout.Config -Encoding utf8NoBOM
    & node (Join-Path $layout.Root 'node_modules/@tauri-apps/cli/tauri.js') build --debug --no-bundle --config $layout.Config
    if ($LASTEXITCODE -ne 0) { throw "隔离 UI 构建失败（退出码 $LASTEXITCODE）。" }
    Copy-Item -LiteralPath (Join-Path $layout.Target 'debug/GlueSight.exe') -Destination $layout.Executable -Force
    @{ identifier = $layout.Identifier; executable = $layout.Executable; builtAt = [DateTime]::UtcNow.ToString('o') } |
        ConvertTo-Json | Set-Content -LiteralPath $layout.BuildManifest -Encoding utf8NoBOM
} finally {
    Pop-Location
    if ($buildLock) { $buildLock.Dispose() }
    if ($runLock) { $runLock.Dispose() }
}
