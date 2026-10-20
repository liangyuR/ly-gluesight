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
    # 默认 watch 模式下编译失败后 tauri dev 会继续等文件变化：窗口已消失，vite 仍占着 1420。
    # --exit-on-panic 让编译失败或应用异常退出时整条链路（含 vite）一起退出。
    & node (Join-Path $root 'node_modules\@tauri-apps\cli\tauri.js') dev --exit-on-panic @args
    exit $LASTEXITCODE
} finally { Pop-Location }
