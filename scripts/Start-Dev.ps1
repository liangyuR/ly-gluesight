#Requires -Version 7.0
param(
    [Parameter(ValueFromRemainingArguments)][string[]]$TauriArgs
)

$ErrorActionPreference = 'Stop'
$root = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))

if ($env:LYFLOW_CORE_DLL) {
    Write-Host "沿用已设置的 LYFLOW_CORE_DLL=$($env:LYFLOW_CORE_DLL)"
} else {
    $output = Join-Path ([IO.Path]::GetTempPath()) "gluesight-lyflow-$PID-$([guid]::NewGuid().ToString('N')).txt"
    try {
        & pwsh -NoProfile -File (Join-Path $PSScriptRoot 'Restore-LyFlowRuntime.ps1') -RepositoryRoot $root -OutputFile $output
        if ($LASTEXITCODE -ne 0) { throw "恢复脚本退出码 $LASTEXITCODE" }
        $native = (Get-Content -LiteralPath $output -Raw).Trim()
        $core = Join-Path $native 'lyflow_core.dll'
        if (-not (Test-Path -LiteralPath $core -PathType Leaf)) { throw "恢复目录缺少 lyflow_core.dll：$native" }
        $env:LYFLOW_CORE_DLL = $core
        Write-Host "lyFlow 锁定版本已就绪：LYFLOW_CORE_DLL=$core"
    } catch {
        Write-Warning "lyFlow 锁定版本恢复失败，继续启动，lyFlow 将显示未加载：$($_.Exception.Message)"
        Write-Warning '配置 NuGet 凭证（LYFLOW_NUGET_TOKEN 或 ~/.ssh/github_nuget_auth.txt）后重新执行 pnpm dev，或在系统设置中指定核心库路径。'
    } finally {
        Remove-Item -LiteralPath $output -ErrorAction SilentlyContinue
    }
}
Write-Host '系统设置里已保存的核心库路径优先于 LYFLOW_CORE_DLL。'

Push-Location -LiteralPath $root
try {
    & node (Join-Path $root 'node_modules\@tauri-apps\cli\tauri.js') dev @TauriArgs
    exit $LASTEXITCODE
} finally { Pop-Location }
