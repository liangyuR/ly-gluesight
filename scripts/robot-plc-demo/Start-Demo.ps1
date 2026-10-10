param(
    [switch]$Build,
    [switch]$ServicesOnly,
    [switch]$OpenConsole,
    [string]$Config = (Join-Path $PSScriptRoot 'demo.config.json'),
    [string]$Recipe
)
. (Join-Path $PSScriptRoot 'Demo-Common.ps1')
if ($Build -and $ServicesOnly) { throw '-Build 与 -ServicesOnly 不能同时使用。' }
$cfg = Get-DemoConfig $Config
$demoRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..\..'))
$demoOutput = $cfg.runtimeDir
$demoExe = Join-Path $demoOutput 'GlueSight-Robot-PLC.exe'
$demoStatePath = Join-Path $demoOutput 'services.json'
$demoRecipe = if ($Recipe) { (Resolve-Path -LiteralPath $Recipe).Path } else { $cfg.robot.recipe }
$expectedProcesses = if ($ServicesOnly) { 2 } else { 4 }
$configHash = (Get-FileHash -LiteralPath $cfg.configPath).Hash
$sourceHash = Get-DemoSourceHash
$recipeHash = (Get-FileHash -LiteralPath $demoRecipe).Hash
New-Item -ItemType Directory -Path $demoOutput -Force | Out-Null
if (Test-Path -LiteralPath $demoStatePath) {
    $previous = Get-Content -LiteralPath $demoStatePath -Raw | ConvertFrom-Json
    $alive = @($previous.processes | Where-Object { Get-DemoOwnedProcess $_ })
    if ($alive.Count -eq $expectedProcesses -and !$Build -and $previous.configHash -eq $configHash -and
        $previous.sourceHash -eq $sourceHash -and $previous.recipeHash -eq $recipeHash -and
        [bool]$previous.servicesOnly -eq [bool]$ServicesOnly) {
        Write-Output "模拟服务已运行：$($cfg.consoleUrl)"
        if ($OpenConsole) { Start-Process -FilePath $cfg.consoleUrl }
        return
    }
    if ($alive.Count) { throw '已有模拟进程正在运行或代码/配置已更新。先运行 sim:stop，再启动。' }
}
if ($Build) {
    Push-Location -LiteralPath $demoRoot
    try {
        & pnpm exec tauri build --debug --no-bundle --config (Join-Path $PSScriptRoot 'tauri.json')
        if ($LASTEXITCODE -ne 0) { throw '演示构建失败' }
        Copy-Item -LiteralPath (Join-Path $demoRoot 'src-tauri\target\debug\GlueSight.exe') -Destination $demoExe -Force
    } finally { Pop-Location }
}
if (!$ServicesOnly -and !(Test-Path -LiteralPath $demoExe)) {
    throw '首次桌面启动请运行 pnpm sim:build；只启动模型服务可运行 pnpm sim:services。'
}
$ports = @($cfg.plc.port, $cfg.robot.port)
if (!$ServicesOnly) { $ports += $cfg.bridge.debugPort }
foreach ($port in $ports) {
    if (Get-NetTCPConnection -State Listen -LocalPort $port -ErrorAction SilentlyContinue) {
        throw "模拟端口 $port 正在使用，请停止原实例或修改配置。"
    }
}
$pythonPath = (Get-Command python -CommandType Application | Select-Object -First 1).Source
$nodePath = if (!$ServicesOnly) { (Get-Command node -CommandType Application | Select-Object -First 1).Source }
$started = [System.Collections.Generic.List[object]]::new()
function Start-DemoProcess([string]$Name, [string]$Executable, [string]$Arguments) {
    $params = @{FilePath=$Executable; WorkingDirectory=$demoRoot; WindowStyle='Hidden'; PassThru=$true;
        RedirectStandardOutput=(Join-Path $demoOutput "$Name.stdout.log"); RedirectStandardError=(Join-Path $demoOutput "$Name.stderr.log")}
    if ($Arguments) { $params.ArgumentList = $Arguments }
    $process = Start-Process @params
    $started.Add(@{name=$Name;pid=$process.Id;executable=$Executable;startedAt=$process.StartTime.ToUniversalTime().ToString('o')})
}
try {
    $configArgs = ' --config "' + $cfg.configPath + '"'
    Start-DemoProcess 'plc' $pythonPath ('-X utf8 "' + (Join-Path $PSScriptRoot 'plc_service.py') + '"' + $configArgs)
    Start-DemoProcess 'robot' $pythonPath ('-X utf8 "' + (Join-Path $PSScriptRoot 'robot_service.py') + '"' + $configArgs + ' --recipe "' + $demoRecipe + '"')
    if (!$ServicesOnly) {
        $oldArgs = $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS
        try {
            $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = "--remote-debugging-port=$($cfg.bridge.debugPort) --remote-debugging-address=127.0.0.1"
            Start-DemoProcess 'app' $demoExe ''
        } finally { $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = $oldArgs }
        Start-DemoProcess 'camera-bridge' $nodePath ('"' + (Join-Path $PSScriptRoot 'camera-bridge.mjs') + '"' + $configArgs + ' --recipe "' + $demoRecipe + '"')
    }
    $ready = $false
    for ($attempt=0; $attempt -lt 80; $attempt++) {
        if (@($started | Where-Object { !(Get-DemoOwnedProcess $_) }).Count) { throw '模拟进程已退出，请检查 stderr.log。' }
        try {
            $status = Invoke-RestMethod -Uri ($cfg.consoleUrl + '/state') -TimeoutSec 1
            $ready = $null -ne $status.plc -and ($ServicesOnly -or $status.bridgeOnline)
        } catch { }
        if ($ready) { break }
        Start-Sleep -Milliseconds 250
    }
    if (!$ready) { throw '模拟服务未能就绪，请检查运行目录中的 stderr.log。' }
    $manifest = @{processes=@($started.ToArray()); console=$cfg.consoleUrl; recipe=$demoRecipe; servicesOnly=[bool]$ServicesOnly;
        configPath=$cfg.configPath; configHash=$configHash; sourceHash=$sourceHash; recipeHash=$recipeHash}
    if (!$ServicesOnly) { $manifest.appSha256 = (Get-FileHash -LiteralPath $demoExe).Hash }
    $manifest | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath $demoStatePath -Encoding utf8
    Write-Output "Robot + PLC 模拟服务已启动：$($cfg.consoleUrl)"
    if ($OpenConsole) { Start-Process -FilePath $cfg.consoleUrl }
    if ($ServicesOnly) {
        Write-Output '当前仅运行 Robot 与 PLC。相机桥未连接时不能开始图像联调；协议回归用 pnpm sim:test。'
    } elseif (!$status.plc.visionReady) {
        Write-Output 'GlueSight 已启动，视觉尚未就绪。首次使用请按本目录 README.md 完成模拟相机、lyFlow 和配方配置。'
    }
} catch {
    foreach ($item in $started) {
        $owned = Get-DemoOwnedProcess $item
        if ($owned) { Stop-Process -InputObject $owned }
    }
    throw
}
