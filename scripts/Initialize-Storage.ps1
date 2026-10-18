#Requires -Version 7.0
param([ValidateSet('Development', 'Production')][string]$Environment = 'Development')

$ErrorActionPreference = 'Stop'
$taskStorageName = if ($Environment -eq 'Development') { 'GlueSight-dev' } else { 'GlueSight' }
$taskStorageBase = [Environment]::GetFolderPath([Environment+SpecialFolder]::CommonApplicationData)
if (!$taskStorageBase) { throw '无法定位 ProgramData。' }
$taskStorageParent = Join-Path $taskStorageBase 'ly'
$taskStorageRoot = Join-Path $taskStorageParent $taskStorageName
foreach ($taskStoragePath in @($taskStorageBase, $taskStorageParent, $taskStorageRoot)) {
    if (Test-Path -LiteralPath $taskStoragePath) {
        $taskStorageItem = Get-Item -LiteralPath $taskStoragePath -Force
        if (!$taskStorageItem.PSIsContainer -or ($taskStorageItem.Attributes -band [IO.FileAttributes]::ReparsePoint)) {
            throw "资料目录必须是普通目录：$taskStoragePath"
        }
    }
}
$taskStorageIdentity = [Security.Principal.WindowsIdentity]::GetCurrent()
$taskStoragePrincipal = [Security.Principal.WindowsPrincipal]::new($taskStorageIdentity)
if (!$taskStoragePrincipal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    throw '请在管理员 PowerShell 中运行 pnpm storage:init；仅目录初始化需要管理员权限。'
}
[IO.Directory]::CreateDirectory($taskStorageRoot) | Out-Null
& icacls.exe $taskStorageRoot /grant '*S-1-5-32-545:(OI)(CI)M'
if ($LASTEXITCODE -ne 0) { throw "资料目录授权失败：$taskStorageRoot" }
Write-Output "资料目录已准备：$taskStorageRoot。没有移动或删除现有资料。"
