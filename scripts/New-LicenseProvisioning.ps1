#Requires -Version 7.0
param(
    [Parameter(Mandatory)][string]$Config,
    [Parameter(Mandatory)][string]$OutputDirectory
)
$ErrorActionPreference = 'Stop'
$configPath = (Resolve-Path -LiteralPath $Config).Path
$destination = [IO.Path]::GetFullPath($OutputDirectory)
if (Test-Path -LiteralPath $destination) { throw '输出目录必须尚不存在，避免覆盖或更改现有目录权限。' }
New-Item -ItemType Directory -Path $destination | Out-Null
if ($IsWindows) {
    $sid = [Security.Principal.WindowsIdentity]::GetCurrent().User.Value
    & icacls.exe $destination /inheritance:r /grant:r "*${sid}:(OI)(CI)F" '*S-1-5-18:(OI)(CI)F' | Out-Null
    if ($LASTEXITCODE -ne 0) { throw '不能限制密码目录访问权限，已停止生成。' }
} else {
    & chmod 700 -- $destination
    if ($LASTEXITCODE -ne 0) { throw '不能限制密码目录权限，已停止生成。' }
}
$manifest = Join-Path $PSScriptRoot '../tools/license-provision/Cargo.toml'
cargo run --locked --manifest-path $manifest -- $configPath $destination
if ($LASTEXITCODE -ne 0) { throw '授权配置生成失败，请检查输入；不会覆盖已有文件。' }
Write-Host "构建配置：$(Join-Path $destination 'license.local.json')"
Write-Host "密码文件：$(Join-Path $destination 'emergency-password.txt')"
