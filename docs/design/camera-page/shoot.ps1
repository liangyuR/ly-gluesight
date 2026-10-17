# 用 Chrome headless 给 01–08 各截一张 1440×900 PNG（同名）。用法：pwsh ./shoot.ps1 [01 03 ...]
param([string[]]$Only)
$dir = $PSScriptRoot
$chrome = "C:\Program Files\Google\Chrome\Application\chrome.exe"
$pages = Get-ChildItem -LiteralPath $dir -Filter "0*.html" | Sort-Object Name
if ($Only) { $pages = $pages | Where-Object { $n = $_.BaseName; $Only | Where-Object { $n.StartsWith($_) } } }
foreach ($p in $pages) {
  $png = Join-Path $dir ($p.BaseName + ".png")
  $url = "file:///" + ($p.FullName -replace "\\", "/")
  & $chrome --headless=new --disable-gpu --hide-scrollbars --force-device-scale-factor=1 --window-size=1440,900 --virtual-time-budget=1500 --screenshot="$png" $url 2>$null | Out-Null
  Write-Output ("{0} -> {1} KB" -f $p.Name, [int]((Get-Item -LiteralPath $png).Length / 1KB))
}
