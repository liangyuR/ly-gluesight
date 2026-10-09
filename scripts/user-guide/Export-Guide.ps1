param(
    [string]$Output,
    [string]$Browser
)
$ErrorActionPreference = 'Stop'
$guideRepo = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..\..'))
$guideHtml = Join-Path $guideRepo 'docs\user-guide\index.html'
if (!$Output) { $Output = Join-Path $guideRepo 'output\pdf\GlueSight-图文使用教程.pdf' }
$Output = [IO.Path]::GetFullPath($Output)
& node (Join-Path $PSScriptRoot 'build-guide.mjs')
if ($LASTEXITCODE -ne 0) { throw '教程生成失败，请先检查截图与内容。' }
if (!$Browser) {
    $candidates = @(
        (Join-Path ${env:ProgramFiles(x86)} 'Microsoft\Edge\Application\msedge.exe'),
        (Join-Path $env:ProgramFiles 'Microsoft\Edge\Application\msedge.exe'),
        (Join-Path $env:ProgramFiles 'Google\Chrome\Application\chrome.exe'),
        (Join-Path $env:LOCALAPPDATA 'Google\Chrome\Application\chrome.exe')
    )
    $Browser = $candidates | Where-Object { Test-Path -LiteralPath $_ } | Select-Object -First 1
}
if (!$Browser -or !(Test-Path -LiteralPath $Browser)) { throw '未找到 Edge 或 Chrome。可用 -Browser 指定浏览器，或打开 HTML 后打印为 PDF。' }
$guideTemp = Join-Path $guideRepo ('tmp\pdfs\guide-export-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $guideTemp,(Split-Path -Parent $Output) -Force | Out-Null
$guideRendered = Join-Path $guideTemp 'rendered.pdf'
$guideUri = [uri]::new($guideHtml).AbsoluteUri
$guideArgs = @('--headless=new','--disable-gpu','--no-first-run','--no-default-browser-check','--no-pdf-header-footer',
    '--disable-extensions','--disable-background-networking','--virtual-time-budget=2500',
    ('--user-data-dir="' + (Join-Path $guideTemp 'profile') + '"'),('--print-to-pdf="' + $guideRendered + '"'),('"' + $guideUri + '"'))
$guideProcess = Start-Process -FilePath $Browser -ArgumentList $guideArgs -WindowStyle Hidden -PassThru `
    -RedirectStandardOutput (Join-Path $guideTemp 'stdout.log') -RedirectStandardError (Join-Path $guideTemp 'stderr.log')
if (!$guideProcess.WaitForExit(45000)) {
    Stop-Process -InputObject $guideProcess
    throw "导出超时，诊断日志保留在 $guideTemp"
}
if (!(Test-Path -LiteralPath $guideRendered) -or (Get-Item -LiteralPath $guideRendered).Length -lt 1000) {
    throw "PDF 未生成，诊断日志保留在 $guideTemp"
}
Copy-Item -LiteralPath $guideRendered -Destination $Output -Force
Write-Output "PDF 已导出：$Output"
