param(
    [ValidateRange(1024, 65535)][int]$DebugPort = 9337,
    [switch]$SkipBuild,
    [switch]$ResetData,
    [ValidateSet('ui', 'p0', 'flow', 'e2e')][string]$Profile = 'ui'
)

$ErrorActionPreference = 'Stop'
$taskUiRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$taskUiConfig = Join-Path $taskUiRoot "tests\native\tauri-$Profile-test.json"
$taskUiOutput = Join-Path $taskUiRoot "output\playwright\$Profile-regression"
$taskUiExecutable = Join-Path $taskUiOutput 'gluesight-ui-tests.exe'
$taskUiTarget = if ($env:CARGO_TARGET_DIR) { [IO.Path]::GetFullPath($env:CARGO_TARGET_DIR) } else { Join-Path $taskUiRoot 'src-tauri\target' }
$taskUiBuild = Join-Path $taskUiTarget 'debug\GlueSight.exe'
$taskUiManifest = Join-Path $taskUiOutput 'instance.json'
$taskUiExisting = @(Get-Process -ErrorAction SilentlyContinue | Where-Object { $_.Path -eq $taskUiExecutable })
if ($taskUiExisting.Count) { throw "隔离 UI 实例仍在运行：$($taskUiExisting.Id -join ', ')。请先完成或关闭该实例。" }
$taskUiOverlay = Get-Content -LiteralPath $taskUiConfig -Raw | ConvertFrom-Json
if ($taskUiOverlay.identifier -ne "com.xyzrobotics.tujiaovision.$Profile-tests" -or $taskUiOverlay.app.windows[0].visible) {
    throw 'UI test configuration must use the isolated application identifier and a hidden window.'
}
if ($ResetData) {
    $taskUiIdentifier = "com.xyzrobotics.tujiaovision.$Profile-tests"
    foreach ($taskUiBase in @($env:APPDATA, $env:LOCALAPPDATA)) {
        if (!$taskUiBase -or !(Test-Path -LiteralPath $taskUiBase -PathType Container)) { throw '无法定位用户数据目录，拒绝清理。' }
        $taskUiData = Join-Path ([IO.Path]::GetFullPath($taskUiBase)) $taskUiIdentifier
        if (!(Test-Path -LiteralPath $taskUiData)) { continue }
        $taskUiItem = Get-Item -LiteralPath $taskUiData -Force
        if (!$taskUiItem.PSIsContainer -or $taskUiItem.LinkType -or $taskUiItem.Name -cne $taskUiIdentifier -or
            [IO.Path]::GetFullPath($taskUiItem.Parent.FullName) -ne [IO.Path]::GetFullPath($taskUiBase) -or $taskUiIdentifier -notmatch '^com\.xyzrobotics\.tujiaovision\.[a-z0-9]+-tests$') {
            throw "拒绝清理非隔离测试数据目录：$taskUiData"
        }
        Remove-Item -LiteralPath $taskUiItem.FullName -Recurse -Force
    }
}
New-Item -ItemType Directory -Path $taskUiOutput -Force | Out-Null
Push-Location -LiteralPath $taskUiRoot
try {
    if (!$SkipBuild) {
        & node (Join-Path $taskUiRoot 'node_modules\@tauri-apps\cli\tauri.js') build --debug --no-bundle --config $taskUiConfig
        if ($LASTEXITCODE -ne 0) { throw '隔离 UI 构建失败' }
        Copy-Item -LiteralPath $taskUiBuild -Destination $taskUiExecutable -Force
    } elseif (!(Test-Path -LiteralPath $taskUiExecutable -PathType Leaf)) {
        throw 'SkipBuild 需要已有的隔离测试可执行文件。'
    }
    if ($SkipBuild) {
        $taskUiPrevious = Get-Content -LiteralPath $taskUiManifest -Raw | ConvertFrom-Json
        if ($taskUiPrevious.identifier -ne $taskUiOverlay.identifier -or $taskUiPrevious.executable -ne $taskUiExecutable) {
            throw '隔离测试实例标识或路径不匹配，请重新构建。'
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
    try {
        $taskUiState = @{ pid = $taskUiProcess.Id; executable = $taskUiExecutable; identifier = $taskUiOverlay.identifier;
            debugPort = $DebugPort; startedAt = $taskUiProcess.StartTime.ToUniversalTime().ToString('o') }
        $taskUiState | ConvertTo-Json | Set-Content -LiteralPath $taskUiManifest -Encoding utf8
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
