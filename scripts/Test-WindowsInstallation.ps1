#Requires -Version 7.0
param(
    [Parameter(Mandatory)][string]$Installer,
    [Parameter(Mandatory)][string]$InstallDir,
    [Parameter(Mandatory)][string]$ReportDir
)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
. (Join-Path $PSScriptRoot 'Runtime-Common.ps1')
if (-not $IsWindows -or [Runtime.InteropServices.RuntimeInformation]::ProcessArchitecture -ne 'X64') {
    throw '安装验证必须在 Windows x64 PowerShell 中执行。'
}
$Installer = (Resolve-Path -LiteralPath $Installer).Path
$InstallDir = [IO.Path]::GetFullPath($InstallDir)
$ReportDir = [IO.Path]::GetFullPath($ReportDir)
if ($InstallDir -match '[\r\n"]') { throw '安装目录包含不支持的字符。' }
if (Test-Path -LiteralPath $InstallDir) { throw '安装验证要求一个尚不存在的新目录。' }
if ($ReportDir.Equals($InstallDir, [StringComparison]::OrdinalIgnoreCase) -or $ReportDir.StartsWith($InstallDir + [IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase)) {
    throw '验证报告必须放在安装目录之外。'
}
New-Item -ItemType Directory -Path $ReportDir -Force | Out-Null
$reportPath = Join-Path $ReportDir 'installation.json'
$report = [ordered]@{
    installer = $Installer
    installedDirectory = $InstallDir
    status = 'failed'
    guiStarted = $false
    hardwareVerified = $false
}
# 测试安装会改写本机的卸载登记、安装位置记录与 GlueSight 快捷方式：先快照，验证完静默卸载、删测试目录，再还原快照，
# 免得本机已装的 GlueSight 被改指到测试目录
$snapshotDir = Join-Path ([IO.Path]::GetTempPath()) ('gluesight-install-snapshot-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $snapshotDir | Out-Null
$registryKeys = @('HKCU\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\GlueSight', 'HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\GlueSight', 'HKCU\SOFTWARE\xyzrobotics\GlueSight', 'HKLM\SOFTWARE\xyzrobotics\GlueSight')
$savedKeys = @(for ($i = 0; $i -lt $registryKeys.Count; $i++) {
    $file = Join-Path $snapshotDir "key-$i.reg"
    & reg.exe query $registryKeys[$i] *> $null
    if ($LASTEXITCODE -eq 0) { & reg.exe export $registryKeys[$i] $file /y *> $null; if ($LASTEXITCODE -eq 0) { $file } }
})
$shortcuts = @([Environment]::GetFolderPath('Programs'), [Environment]::GetFolderPath('CommonPrograms'), [Environment]::GetFolderPath('Desktop'), [Environment]::GetFolderPath('CommonDesktopDirectory')) |
    ForEach-Object { Join-Path $_ 'GlueSight.lnk' }
$savedShortcuts = @(for ($i = 0; $i -lt $shortcuts.Count; $i++) {
    if (Test-Path -LiteralPath $shortcuts[$i] -PathType Leaf) { $copy = Join-Path $snapshotDir "lnk-$i.lnk"; Copy-Item -LiteralPath $shortcuts[$i] -Destination $copy; [pscustomobject]@{ path = $shortcuts[$i]; copy = $copy } }
})
try {
    $processInfo = [Diagnostics.ProcessStartInfo]::new()
    $processInfo.FileName = $Installer
    $processInfo.Arguments = "/S /D=$InstallDir"
    $processInfo.UseShellExecute = $false
    $processInfo.CreateNoWindow = $true
    $process = [Diagnostics.Process]::Start($processInfo)
    try {
        if (-not $process.WaitForExit(180000)) {
            $process.Kill($true)
            throw '静默安装在 180 秒内未完成。'
        }
        if ($process.ExitCode -ne 0) { throw "静默安装失败，退出码 $($process.ExitCode)。" }
        $report.installerExitCode = $process.ExitCode
    } finally { $process.Dispose() }
    foreach ($relative in @('GlueSight.exe', 'runtime/manifest.json', 'runtime/lyflow/lyflow_core.dll', 'runtime/mvs/MvCameraControl.dll')) {
        if (-not (Test-Path -LiteralPath (Join-Path $InstallDir $relative) -PathType Leaf)) { throw "安装目录缺少 $relative。" }
    }
    foreach ($name in @('vcruntime140.dll', 'vcruntime140_1.dll')) {
        $path = Join-Path $InstallDir $name
        if (-not (Test-Path -LiteralPath $path -PathType Leaf)) { throw "安装目录缺少 $name。" }
        Assert-NoRuntimeLinks $path
        Assert-X64RuntimeImage $path
    }
    $webViewInstallers = @(Get-ChildItem -LiteralPath $InstallDir -File -Recurse | Where-Object { $_.Name -match '(?i)(webview.*(setup|installer)|(setup|installer).*webview).*\.exe$' })
    if ($webViewInstallers.Count -ne 0) { throw '安装目录不应携带 WebView2 安装程序。' }
    $runtimeReport = Join-Path $ReportDir 'installed-runtime.json'
    & pwsh -NoProfile -File (Join-Path $PSScriptRoot 'Test-PackagedRuntime.ps1') -RuntimeRoot (Join-Path $InstallDir 'runtime') -ReportPath $runtimeReport
    if ($LASTEXITCODE -ne 0) { throw '安装后的运行库自检失败。' }
    $applicationReport = Join-Path $ReportDir 'installed-application.json'
    $applicationInfo = [Diagnostics.ProcessStartInfo]::new()
    $applicationInfo.FileName = Join-Path $InstallDir 'GlueSight.exe'
    $applicationInfo.UseShellExecute = $false
    $applicationInfo.CreateNoWindow = $true
    $applicationInfo.WorkingDirectory = $ReportDir
    $applicationInfo.ArgumentList.Add('--check-packaged-runtime')
    $applicationInfo.ArgumentList.Add($applicationReport)
    $application = [Diagnostics.Process]::Start($applicationInfo)
    try {
        if (-not $application.WaitForExit(120000)) {
            $application.Kill($true)
            throw '安装后的应用运行库自检在 120 秒内未完成。'
        }
        if ($application.ExitCode -ne 0) { throw "安装后的应用运行库自检失败，退出码 $($application.ExitCode)。" }
    } finally { $application.Dispose() }
    if (-not (Test-Path -LiteralPath $applicationReport -PathType Leaf)) { throw '应用运行库自检未生成报告。' }
    $report.runtimeReport = $runtimeReport
    $report.applicationReport = $applicationReport
    $report.status = 'passed'
    Write-Host '静默安装验证通过：应用和两套运行库部署齐全，已通过安装后的运行库自检。'
} catch {
    $report.error = $_.Exception.Message
    throw
} finally {
    # 静默卸载：_?= 让卸载程序就地同步运行（不复制到临时目录后立即返回），之后由这里删掉测试目录
    $uninstaller = Join-Path $InstallDir 'uninstall.exe'
    $report.uninstalled = $false
    if (Test-Path -LiteralPath $uninstaller -PathType Leaf) {
        $uninstall = Start-Process -FilePath $uninstaller -ArgumentList '/S', "_?=$InstallDir" -PassThru -WindowStyle Hidden
        if ($uninstall.WaitForExit(180000)) { $report.uninstalled = $uninstall.ExitCode -eq 0 } else { $uninstall.Kill($true) }
        if (-not $report.uninstalled) { Write-Warning "测试安装的静默卸载未成功，请手动检查：$InstallDir" }
    }
    if (Test-Path -LiteralPath $InstallDir) {
        Remove-Item -LiteralPath $InstallDir -Recurse -Force -ErrorAction Continue
    }
    $report.installedDirectoryRemoved = -not (Test-Path -LiteralPath $InstallDir)
    foreach ($key in $registryKeys) { & reg.exe delete $key /f *> $null }
    foreach ($file in $savedKeys) { & reg.exe import $file *> $null; if ($LASTEXITCODE -ne 0) { Write-Warning "未能还原注册表快照 $file" } }
    foreach ($path in $shortcuts) { if (Test-Path -LiteralPath $path) { Remove-Item -LiteralPath $path -Force -ErrorAction Continue } }
    foreach ($item in $savedShortcuts) { Copy-Item -LiteralPath $item.copy -Destination $item.path -Force -ErrorAction Continue }
    Remove-Item -LiteralPath $snapshotDir -Recurse -Force -ErrorAction SilentlyContinue
    $report | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $reportPath -Encoding utf8NoBOM
}
