# 给某一版目录下的状态页截 PNG（同名）。默认 1440×900；文件名含 "1280" 的按 1280×800。用法：pwsh ./shoot.ps1 v1 [a b ...]
param([Parameter(Mandatory)][string]$Version, [string[]]$Only)
$dir = Join-Path $PSScriptRoot $Version
$chrome = "C:\Program Files\Google\Chrome\Application\chrome.exe"
$pages = Get-ChildItem -LiteralPath $dir -Filter "*.html" | Sort-Object Name
if ($Only) { $pages = $pages | Where-Object { $n = $_.BaseName; $Only | Where-Object { $n.StartsWith($_) } } }
foreach ($p in $pages) {
  $png = Join-Path $dir ($p.BaseName + ".png")
  $url = "file:///" + ($p.FullName -replace "\\", "/")
  $size = if ($p.BaseName -match "1280") { "1280,800" } else { "1440,900" }
  & $chrome --headless=new --disable-gpu --hide-scrollbars --force-device-scale-factor=1 --window-size=$size --virtual-time-budget=1500 --screenshot="$png" $url 2>$null | Out-Null
  Write-Output ("{0} -> {1} KB" -f $p.Name, [int]((Get-Item -LiteralPath $png).Length / 1KB))
}
