param(
    [ValidateRange(1024, 65535)][int]$DebugPort = 9337,
    [switch]$SkipBuild
)

$ErrorActionPreference = 'Stop'
$taskUiRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$taskUiConfig = Join-Path $taskUiRoot 'tests\native\tauri-ui-test.json'
$taskUiOutput = Join-Path $taskUiRoot 'output\playwright\ui-regression'
$taskUiExecutable = Join-Path $taskUiOutput 'gluesight-ui-tests.exe'
$taskUiBuild = Join-Path $taskUiRoot 'src-tauri\target\debug\GlueSight.exe'
$taskUiManifest = Join-Path $taskUiOutput 'instance.json'
$taskUiExisting = @(Get-Process -ErrorAction SilentlyContinue | Where-Object { $_.Path -eq $taskUiExecutable })
if ($taskUiExisting.Count) { throw "隔离 UI 实例仍在运行：$($taskUiExisting.Id -join ', ')。请先完成或关闭该实例。" }
$taskUiOverlay = Get-Content -LiteralPath $taskUiConfig -Raw | ConvertFrom-Json
if ($taskUiOverlay.identifier -ne 'com.xyzrobotics.tujiaovision.ui-tests' -or $taskUiOverlay.app.windows[0].visible) {
    throw 'UI test configuration must use the isolated application identifier and a hidden window.'
}
New-Item -ItemType Directory -Path $taskUiOutput -Force | Out-Null
Push-Location -LiteralPath $taskUiRoot
try {
    if (!$SkipBuild) {
        & pnpm exec tauri build --debug --no-bundle --config $taskUiConfig
        if ($LASTEXITCODE -ne 0) { throw '隔离 UI 构建失败' }
        Copy-Item -LiteralPath $taskUiBuild -Destination $taskUiExecutable -Force
    } elseif (!(Test-Path -LiteralPath $taskUiExecutable -PathType Leaf)) {
        throw 'SkipBuild 需要已有的隔离测试可执行文件。'
    }
    if ($SkipBuild) {
        $taskUiPrevious = Get-Content -LiteralPath $taskUiManifest -Raw | ConvertFrom-Json
        if ($taskUiPrevious.identifier -ne $taskUiOverlay.identifier -or $taskUiPrevious.sha256 -ne (Get-FileHash -LiteralPath $taskUiExecutable -Algorithm SHA256).Hash) {
            throw '隔离测试实例标识或文件哈希不匹配，请重新构建。'
        }
    }
    $taskUiProbe = [Net.Sockets.TcpListener]::new([Net.IPAddress]::Loopback, $DebugPort)
    try { $taskUiProbe.Start() } finally { $taskUiProbe.Stop() }
    $taskUiOldArguments = $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS
    try {
        $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = "--remote-debugging-port=$DebugPort"
        $taskUiProcess = Start-Process -FilePath $taskUiExecutable -WorkingDirectory $taskUiRoot -WindowStyle Hidden -PassThru `
            -RedirectStandardOutput (Join-Path $taskUiOutput 'stdout.log') -RedirectStandardError (Join-Path $taskUiOutput 'stderr.log')
    } finally { $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = $taskUiOldArguments }
    $taskUiState = @{ pid = $taskUiProcess.Id; executable = $taskUiExecutable; identifier = $taskUiOverlay.identifier;
        sha256 = (Get-FileHash -LiteralPath $taskUiExecutable -Algorithm SHA256).Hash; debugPort = $DebugPort; startedAt = [DateTime]::UtcNow.ToString('o') }
    $taskUiState | ConvertTo-Json | Set-Content -LiteralPath $taskUiManifest -Encoding utf8
    try {
        $taskUiReady = $false
        for ($taskUiAttempt = 0; $taskUiAttempt -lt 40; $taskUiAttempt++) {
            try { $taskUiVersion = Invoke-RestMethod -Uri "http://127.0.0.1:$DebugPort/json/version" -TimeoutSec 1; $taskUiReady = [bool]$taskUiVersion.webSocketDebuggerUrl } catch { }
            if ($taskUiReady) { break }
            if ($taskUiProcess.HasExited) { throw '隔离 UI 程序在浏览器初始化前退出' }
            Start-Sleep -Milliseconds 250
        }
        if (!$taskUiReady) { throw "隔离 UI 程序未准备好；检查 $taskUiOutput 的日志。" }
    } catch {
        $taskUiOwnedProcess = Get-Process -Id $taskUiProcess.Id -ErrorAction SilentlyContinue
        if ($taskUiOwnedProcess -and $taskUiOwnedProcess.Path -eq $taskUiExecutable) { Stop-Process -InputObject $taskUiOwnedProcess }
        throw
    }
    $taskUiState | ConvertTo-Json -Compress
} finally { Pop-Location }
