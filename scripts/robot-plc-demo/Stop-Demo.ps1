param(
    [string]$Config = (Join-Path $PSScriptRoot 'demo.config.json'),
    [ValidateRange(1,300)][int]$TimeoutSeconds = 60
)
. (Join-Path $PSScriptRoot 'Demo-Common.ps1')
$cfg = Get-DemoConfig $Config
$demoStatePath = Join-Path $cfg.runtimeDir 'services.json'
if (!(Test-Path -LiteralPath $demoStatePath)) { Write-Output '没有已登记的模拟进程。'; return }
$manifest = Get-Content -LiteralPath $demoStatePath -Raw | ConvertFrom-Json
$robotItem = $manifest.processes | Where-Object name -eq 'robot' | Select-Object -First 1
if ($robotItem -and (Get-DemoOwnedProcess $robotItem)) {
    try {
        $status = Invoke-RestMethod -Uri ($manifest.console + '/state') -TimeoutSec 2
        if ($status.running) {
            $null = Invoke-RestMethod -Uri ($manifest.console + '/stop') -Method Post -ContentType 'application/json' -Body '{}'
            $deadline = [datetime]::UtcNow.AddSeconds($TimeoutSeconds)
            do {
                Start-Sleep -Milliseconds 250
                $status = Invoke-RestMethod -Uri ($manifest.console + '/state') -TimeoutSec 2
            } while ($status.running -and [datetime]::UtcNow -lt $deadline)
            if ($status.running) { throw '本件尚未结束，暂不关闭。可增加 -TimeoutSeconds 后重试。' }
        }
    } catch {
        if ($_.Exception.Message.StartsWith('本件尚未结束')) { throw }
        Write-Warning 'Robot 接口不可达，将关闭本实例登记的进程。'
    }
}
foreach ($name in @('camera-bridge','robot','app','plc')) {
    foreach ($item in @($manifest.processes | Where-Object name -eq $name)) {
        $owned = Get-DemoOwnedProcess $item
        if ($owned) {
            Stop-Process -InputObject $owned
            $owned.WaitForExit(5000) | Out-Null
        }
    }
}
Write-Output '模拟进程已停止，配置、原图和历史记录保留。'
