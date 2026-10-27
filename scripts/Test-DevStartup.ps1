#Requires -Version 7.0
$ErrorActionPreference = 'Stop'
if (-not $IsWindows) { throw '此集成验证仅支持 Windows。' }
$root = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$fixture = Join-Path $root "output/dev-startup-$([Guid]::NewGuid().ToString('N'))"
$name = 'GLUESIGHT_LYFLOW_RUNTIME'
$saved = [Environment]::GetEnvironmentVariable($name, 'User')
$inherited = [Environment]::GetEnvironmentVariable($name, 'Process')

try {
    $scripts = Join-Path $fixture 'scripts'
    $cli = Join-Path $fixture 'node_modules/@tauri-apps/cli'
    $runtime = Join-Path $fixture 'local runtime'
    foreach ($dir in @($scripts, $cli, $runtime)) {
        [void][IO.Directory]::CreateDirectory($dir)
    }
    Copy-Item -LiteralPath (Join-Path $PSScriptRoot 'Start-Dev.ps1') -Destination $scripts
    Set-Content -LiteralPath (Join-Path $runtime 'lyflow_core.dll') -Value ''
    @'
console.log(JSON.stringify({ runtime: process.env.GLUESIGHT_LYFLOW_RUNTIME ?? null, args: process.argv.slice(2) }));
process.exit(17);
'@ | Set-Content -LiteralPath (Join-Path $cli 'tauri.js')

    foreach ($expected in @($runtime, $null)) {
        if ($null -eq $expected) {
            & (Join-Path $PSScriptRoot 'Set-LocalLyFlowRuntime.ps1') -Clear
        } else {
            & (Join-Path $PSScriptRoot 'Set-LocalLyFlowRuntime.ps1') $expected
        }
        $env:GLUESIGHT_LYFLOW_RUNTIME = 'C:\stale-terminal-runtime'
        $output = @(& pwsh -NoProfile -File (Join-Path $scripts 'Start-Dev.ps1') --no-watch --config 'config with spaces.json')
        if ($LASTEXITCODE -ne 17) { throw '启动器没有保留子进程退出码。' }
        $result = $output[-1] | ConvertFrom-Json
        if ($result.runtime -cne $expected) { throw "旧终端覆盖了保存的配置：$($result.runtime)" }
        if (($result.args | ConvertTo-Json -Compress) -cne '["dev","--exit-on-panic","--no-watch","--config","config with spaces.json"]') {
            throw '启动参数未完整传递。'
        }
    }
    Write-Host 'PASS: 旧终端在设置或清除配置后启动 → 子进程使用最新保存值，参数与退出码保持不变。'
} finally {
    $restoreSaved = if ($null -eq $saved) { [NullString]::Value } else { $saved }
    [Environment]::SetEnvironmentVariable($name, $restoreSaved, 'User')
    $restoreInherited = if ($null -eq $inherited) { [NullString]::Value } else { $inherited }
    [Environment]::SetEnvironmentVariable($name, $restoreInherited, 'Process')
}
