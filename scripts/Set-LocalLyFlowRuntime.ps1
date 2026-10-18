#Requires -Version 7.0
[CmdletBinding(DefaultParameterSetName = 'Show')]
param(
    [Parameter(Mandatory, Position = 0, ParameterSetName = 'Set')]
    [ValidateNotNullOrEmpty()]
    [string]$RuntimePath,
    [Parameter(Mandatory, ParameterSetName = 'Clear')]
    [switch]$Clear,
    [Parameter(ParameterSetName = 'Show')]
    [switch]$Show
)

$ErrorActionPreference = 'Stop'
if (-not $IsWindows) { throw '本地 LyFlow 运行库配置仅支持 Windows。' }
$name = 'GLUESIGHT_LYFLOW_RUNTIME'

switch ($PSCmdlet.ParameterSetName) {
    'Set' {
        $runtime = (Resolve-Path -LiteralPath $RuntimePath).ProviderPath
        if (-not (Test-Path -LiteralPath $runtime -PathType Container) -or
            -not (Test-Path -LiteralPath (Join-Path $runtime 'lyflow_core.dll') -PathType Leaf)) {
            throw "运行库目录缺少 lyflow_core.dll：$runtime"
        }
        [Environment]::SetEnvironmentVariable($name, $runtime, 'User')
        Write-Host "已固定本地 LyFlow 运行库：$runtime"
    }
    'Clear' {
        [Environment]::SetEnvironmentVariable($name, [NullString]::Value, 'User')
        Write-Host '已清除用户级本地 LyFlow 运行库配置。'
    }
    'Show' {
        $saved = [Environment]::GetEnvironmentVariable($name, 'User')
        $current = [Environment]::GetEnvironmentVariable($name, 'Process')
        Write-Host "用户级保存值：$(if ($saved) { $saved } else { '未设置' })"
        Write-Host "当前进程值：$(if ($current) { $current } else { '未设置' })"
        return
    }
}

Write-Host '请重开终端；从 IDE 启动时请重启 IDE，然后运行 pnpm dev。已运行的 GlueSight 也需要重启。'
Write-Host '仅开发构建生效；dev:joint -LyFlowRuntime 的显式目录仍优先于此配置。'
