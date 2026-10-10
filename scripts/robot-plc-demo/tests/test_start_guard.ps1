$ErrorActionPreference = 'Stop'
$source = Join-Path $PSScriptRoot '..\Start-Demo.ps1'
$tokens = $null
$errors = $null
$ast = [Management.Automation.Language.Parser]::ParseFile($source, [ref]$tokens, [ref]$errors)
if ($errors.Count) { throw ($errors | Out-String) }
$guard = @($ast.FindAll({ param($node)
    $node -is [Management.Automation.Language.IfStatementAst] -and $node.Clauses[0].Item1.Extent.Text -eq 'Test-Path -LiteralPath $demoStatePath'
}, $true))
if ($guard.Count -ne 1) { throw 'Expected one services.json active-process guard' }
$block = [scriptblock]::Create($guard[0].Extent.Text)
$demoStatePath = 'C:\test-only\services.json'
$cfg = @{configPath='C:\test-only\demo.config.json';consoleUrl='http://127.0.0.1:18777'}
$demoRecipe = 'C:\test-only\recipe.json'
$ServicesOnly = $false
$Build = $false
$OpenConsole = $false
function Test-Path { param($LiteralPath) return $true }
function Get-Content { param($LiteralPath, [switch]$Raw) return ($script:manifest | ConvertTo-Json -Depth 5) }
function Get-DemoOwnedProcess { param($Item) if ($Item.alive) { return $Item } }
$cases = 0
foreach ($aliveCount in @(0, 1, 4)) {
    $script:manifest = @{configPath=$cfg.configPath;recipe=$demoRecipe;servicesOnly=$false;processes=@(1..4 | ForEach-Object { @{alive=($_ -le $aliveCount);pid=$_} })}
    $failure = $null
    try { & $block } catch { $failure = $_.Exception.Message }
    if ($aliveCount -eq 0) {
        if ($failure) { throw "Exited processes must allow fresh startup: $failure" }
    } elseif (!$failure -or !$failure.Contains('Stop-Demo.ps1')) { throw 'Live matching or partial instances must refuse reuse and require normal stop' }
    $cases++
}
Write-Output "Start-Demo active-instance guard: $cases pure behavior checks passed; no process started or stopped"
