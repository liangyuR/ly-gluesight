$ErrorActionPreference = 'Stop'

function Get-DemoConfig([string]$Config) {
    $python = (Get-Command python -CommandType Application | Select-Object -First 1).Source
    $json = & $python -X utf8 (Join-Path $PSScriptRoot 'simulator_config.py') --config $Config
    if ($LASTEXITCODE -ne 0) { throw '模拟配置无效，请检查上面的错误。' }
    return ($json | ConvertFrom-Json)
}

function Get-DemoOwnedProcess($Item) {
    $process = Get-Process -Id $Item.pid -ErrorAction SilentlyContinue
    if ($process -and $process.Path -eq $Item.executable -and
        $process.StartTime.ToUniversalTime().Ticks -eq ([datetime]$Item.startedAt).ToUniversalTime().Ticks) {
        return $process
    }
}
