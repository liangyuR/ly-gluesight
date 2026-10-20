#Requires -Version 7.0
param(
    [string]$LyFlowRuntime,
    [string]$LyFlowSource,
    [Parameter(ValueFromRemainingArguments)][string[]]$TauriArgs
)

$ErrorActionPreference = 'Stop'
$root = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$TauriArgs = @($TauriArgs | Where-Object { $null -ne $_ })

function Stop-Dev([string]$Message) {
    [Console]::Error.WriteLine($Message)
    exit 1
}

if (-not $LyFlowRuntime -and -not $LyFlowSource) { Stop-Dev '联合开发至少指定 -LyFlowRuntime 或 -LyFlowSource；普通开发直接用 pnpm dev。' }

if ($LyFlowRuntime) {
    $runtimePath = [IO.Path]::GetFullPath($LyFlowRuntime)
    if (-not (Test-Path -LiteralPath (Join-Path $runtimePath 'lyflow_core.dll') -PathType Leaf)) { Stop-Dev "-LyFlowRuntime 目录缺少 lyflow_core.dll：$runtimePath" }
    $env:GLUESIGHT_LYFLOW_RUNTIME = $runtimePath
    Write-Warning "联合开发：本次启动使用 GLUESIGHT_LYFLOW_RUNTIME=$runtimePath（仅当前进程，不落盘）。"
}

if ($LyFlowSource) {
    $sourceRoot = [IO.Path]::GetFullPath($LyFlowSource)
    $client = Join-Path $sourceRoot 'crates/lyflow-client'
    if (-not (Test-Path -LiteralPath (Join-Path $client 'Cargo.toml') -PathType Leaf)) { Stop-Dev "-LyFlowSource 不是 lyFlow 仓库根目录（缺少 crates/lyflow-client/Cargo.toml）：$sourceRoot" }
    . (Join-Path $PSScriptRoot 'LyFlow-Dependency.ps1')
    $gitUrl = Get-LyFlowGitUrl (Get-LyFlowDependencyConfig $root)
    $patch = "patch.`"$gitUrl`".lyflow-client.path='$($client.Replace('\', '/'))'"
    # 本地源码版本与锁定版本不同时 cargo 只记 patch.unused、仍用 Release 客户端，先把锁切到本地源码
    & cargo update -p lyflow-client --manifest-path (Join-Path $root 'src-tauri/Cargo.toml') --config $patch
    if ($LASTEXITCODE -ne 0) { Stop-Dev "lyflow-client 切换到本地源码失败（退出码 $LASTEXITCODE）：$client" }
    $separator = [Array]::IndexOf($TauriArgs, '--')
    $TauriArgs = if ($separator -ge 0) {
        @($TauriArgs[0..$separator]) + @('--config', $patch) + @($TauriArgs | Select-Object -Skip ($separator + 1))
    } else { @($TauriArgs) + @('--', '--config', $patch) }
    Write-Warning "联合开发：lyflow-client 改用本地源码 $client。本地 src-tauri/Cargo.lock 会变化，结束后执行 git checkout -- src-tauri/Cargo.lock，不要提交。"
}

Push-Location -LiteralPath $root
try {
    # 编译失败或应用异常退出时整条链路（含 vite）一起退出，原因见 Start-Dev.ps1。
    & node (Join-Path $root 'node_modules\@tauri-apps\cli\tauri.js') dev --exit-on-panic @TauriArgs
    exit $LASTEXITCODE
} finally { Pop-Location }
