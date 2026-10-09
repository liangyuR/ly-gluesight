param(
    [ValidateSet('Glue1','Glue2')][string]$Group = 'Glue1',
    [ValidateRange(1024,65535)][int]$DebugPort = 9349
)
$ErrorActionPreference = 'Stop'
$demoPackage = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$demoConfig = $PSScriptRoot
$demoInfo = Get-Content -LiteralPath (Join-Path $demoConfig 'application.json') -Raw | ConvertFrom-Json
if ($demoInfo.identifier -ne 'com.xyzrobotics.gluesight.offline-demo-20261009') { throw 'Unexpected demo profile' }
$demoExe = Join-Path $demoConfig 'runner\GlueSight-Offline.exe'
if ((Get-FileHash -LiteralPath $demoExe -Algorithm SHA256).Hash -ne $demoInfo.sha256) { throw 'Demo executable hash mismatch' }
$demoExisting = @(Get-Process -ErrorAction SilentlyContinue | Where-Object { $_.Path -eq $demoExe })
if ($demoExisting.Count) { throw '离线演示已打开。切换案例前，请先关闭当前离线演示窗口。' }
$demoProfile = Join-Path $env:APPDATA $demoInfo.identifier
New-Item -ItemType Directory -Path $demoProfile -Force | Out-Null
$demoPreset = Join-Path $demoConfig $Group
$demoCameras = Get-Content -LiteralPath (Join-Path $demoPreset 'cameras.json') -Raw | ConvertFrom-Json
foreach ($camera in $demoCameras.cameras) { $camera.replayDir = Join-Path $demoPackage $Group }
$demoCameras | ConvertTo-Json -Depth 30 | Set-Content -LiteralPath (Join-Path $demoProfile 'cameras.json') -Encoding utf8NoBOM
$demoCyclePath = Join-Path $demoProfile 'cycle.json'
if (-not (Test-Path -LiteralPath $demoCyclePath)) {
    Copy-Item -LiteralPath (Join-Path $demoConfig 'cycle.json') -Destination $demoCyclePath
}
$demoRecipeDir = Join-Path $demoProfile 'recipes'
New-Item -ItemType Directory -Path $demoRecipeDir -Force | Out-Null
foreach ($file in Get-ChildItem -LiteralPath (Join-Path $demoConfig 'recipes') -Filter '*.json' -File) {
    $destination = Join-Path $demoRecipeDir $file.Name
    if (-not (Test-Path -LiteralPath $destination)) { Copy-Item -LiteralPath $file.FullName -Destination $destination }
}
$demoProbe = [Net.Sockets.TcpListener]::new([Net.IPAddress]::Loopback,$DebugPort)
try { $demoProbe.Start() } finally { $demoProbe.Stop() }
$demoLog = Join-Path $demoPackage '复测结果'
$demoPreviousArgs = $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS
try {
    $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = "--remote-debugging-port=$DebugPort"
    $demoProcess = Start-Process -FilePath $demoExe -WorkingDirectory $demoConfig -WindowStyle Normal -PassThru `
        -RedirectStandardOutput (Join-Path $demoLog 'application-stdout.log') `
        -RedirectStandardError (Join-Path $demoLog 'application-stderr.log')
} finally { $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = $demoPreviousArgs }
$demoState = @{ identifier=$demoInfo.identifier; group=$Group; pid=$demoProcess.Id; executable=$demoExe;
    profile=$demoProfile; debugPort=$DebugPort; startedAt=[DateTime]::UtcNow.ToString('o'); sha256=$demoInfo.sha256 }
$demoState | ConvertTo-Json | Set-Content -LiteralPath (Join-Path $demoLog 'instance.json') -Encoding utf8NoBOM
Write-Output "已打开 $Group 离线演示。比例尺为演示像素尺度；相机回放及结果保存已准备。"
