#Requires -Version 7.0
param(
    [switch]$NoLyFlow,
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

if ($NoLyFlow -and $LyFlowRuntime) { Stop-Dev '-NoLyFlow 与 -LyFlowRuntime 不能同时使用。' }
$targetRoot = if ($env:CARGO_TARGET_DIR) {
    if ([IO.Path]::IsPathRooted($env:CARGO_TARGET_DIR)) { [IO.Path]::GetFullPath($env:CARGO_TARGET_DIR) }
    else { [IO.Path]::GetFullPath((Join-Path $root "src-tauri/$env:CARGO_TARGET_DIR")) }
} else { Join-Path $root 'src-tauri/target' }
$debugRuntime = Join-Path $targetRoot 'debug/runtime/lyflow'

if ($NoLyFlow) {
    if (Test-Path -LiteralPath $debugRuntime) { Remove-Item -LiteralPath $debugRuntime -Recurse -Force }
    Write-Warning "已按 -NoLyFlow 跳过 lyFlow 运行库并清空 $debugRuntime；本次启动没有图像测量引擎。"
} elseif ($LyFlowRuntime) {
    $runtimePath = [IO.Path]::GetFullPath($LyFlowRuntime)
    if (-not (Test-Path -LiteralPath (Join-Path $runtimePath 'lyflow_core.dll') -PathType Leaf)) { Stop-Dev "-LyFlowRuntime 目录缺少 lyflow_core.dll：$runtimePath" }
    $env:GLUESIGHT_LYFLOW_RUNTIME = $runtimePath
    Write-Warning "联合开发：本次启动使用 GLUESIGHT_LYFLOW_RUNTIME=$runtimePath（仅当前进程，不落盘）。"
} else {
    $output = Join-Path ([IO.Path]::GetTempPath()) "gluesight-lyflow-$PID-$([guid]::NewGuid().ToString('N')).json"
    try {
        & pwsh -NoProfile -File (Join-Path $PSScriptRoot 'Restore-LyFlowRuntime.ps1') -RepositoryRoot $root -OutputFile $output
        if ($LASTEXITCODE -ne 0 -or -not (Test-Path -LiteralPath $output -PathType Leaf)) {
            Stop-Dev "lyFlow 运行库恢复失败（退出码 $LASTEXITCODE），已停止启动。请检查网络或上方错误；如需无引擎启动，执行 pnpm dev -NoLyFlow。"
        }
        $runtime = Get-Content -LiteralPath $output -Raw | ConvertFrom-Json
    } finally { Remove-Item -LiteralPath $output -ErrorAction SilentlyContinue }
    if (Test-Path -LiteralPath $debugRuntime) { Remove-Item -LiteralPath $debugRuntime -Recurse -Force }
    [void][IO.Directory]::CreateDirectory($debugRuntime)
    foreach ($item in Get-ChildItem -LiteralPath $runtime.path -Force) { Copy-Item -LiteralPath $item.FullName -Destination $debugRuntime -Recurse -Force }
    Write-Host "lyFlow $($runtime.version) 已复制到 $debugRuntime"
}

if (-not $LyFlowSource) {
    $lock = Get-Content -LiteralPath (Join-Path $root 'src-tauri/Cargo.lock') -Raw
    if ($lock -match '(?ms)^name = "lyflow-client"\r?\nversion = "[^"]+"\r?\n(?!source = "git\+)') {
        Stop-Dev 'src-tauri/Cargo.lock 仍指向联合开发时的本地 lyflow-client。执行 git checkout -- src-tauri/Cargo.lock 恢复后再启动，或继续加 -LyFlowSource。'
    }
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
    Write-Warning "联合开发：lyflow-client 改用本地源码 $client。本地 src-tauri/Cargo.lock 会变化，不要提交。"
}

Push-Location -LiteralPath $root
try {
    & node (Join-Path $root 'node_modules\@tauri-apps\cli\tauri.js') dev @TauriArgs
    exit $LASTEXITCODE
} finally { Pop-Location }
