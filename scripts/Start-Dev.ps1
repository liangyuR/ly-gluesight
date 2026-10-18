#Requires -Version 7.0
$ErrorActionPreference = 'Stop'
$root = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))

if ($IsWindows) {
    $runtime = [Environment]::GetEnvironmentVariable('GLUESIGHT_LYFLOW_RUNTIME', 'User')
    if (-not [string]::IsNullOrWhiteSpace($runtime)) {
        if (-not (Test-Path -LiteralPath (Join-Path $runtime 'lyflow_core.dll') -PathType Leaf)) {
            throw "已保存的本地 LyFlow 目录缺少 lyflow_core.dll：$runtime。请用 pnpm deps:lyflow:local 重新设置，或加 -Clear 清除。"
        }
        $env:GLUESIGHT_LYFLOW_RUNTIME = $runtime
        Write-Host "本地 LyFlow 运行库：$runtime"
    } else {
        Remove-Item -LiteralPath Env:\GLUESIGHT_LYFLOW_RUNTIME -ErrorAction SilentlyContinue
    }
}

Push-Location -LiteralPath $root
try {
    & node (Join-Path $root 'node_modules\@tauri-apps\cli\tauri.js') dev @args
    exit $LASTEXITCODE
} finally { Pop-Location }
