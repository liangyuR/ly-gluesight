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

function Get-DemoSourceHash {
    $hashes = Get-ChildItem -LiteralPath $PSScriptRoot -File |
        Where-Object Extension -In '.py', '.mjs', '.html', '.ps1' |
        Sort-Object Name | ForEach-Object { (Get-FileHash -LiteralPath $_.FullName).Hash }
    $bytes = [Text.Encoding]::UTF8.GetBytes(($hashes -join ''))
    return [Convert]::ToHexString([Security.Cryptography.SHA256]::HashData($bytes))
}
