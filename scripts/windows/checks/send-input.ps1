param(
    [string]$File = $env:MEDIAA_DEV_INPUT,
    [ValidateSet('overlay', 'root')][string]$Viewport = 'overlay',
    [string]$Move = "",
    [string]$Click = "",
    [string]$Drag = "",
    [string]$Key = "",
    [string]$Text = "",
    [int]$DelayMs = 120,
    [switch]$Clear
)

$ErrorActionPreference = "Stop"
if ([string]::IsNullOrWhiteSpace($File)) {
    throw "set MEDIAA_DEV_INPUT or pass -File"
}
$File = [IO.Path]::GetFullPath($File)
New-Item -ItemType Directory -Force (Split-Path -Parent $File) | Out-Null
if ($Clear) {
    [IO.File]::WriteAllText($File, "", (New-Object Text.UTF8Encoding($false)))
    "cleared $File"
    return
}

function Write-Line($payload) {
    $line = ($payload | ConvertTo-Json -Compress) + "`n"
    [IO.File]::AppendAllText($File, $line, (New-Object Text.UTF8Encoding($false)))
}

function Read-Point([string]$text) {
    $parts = $text.Split(',')
    if ($parts.Length -ne 2) { throw "expected x,y but got '$text'" }
    return [pscustomobject]@{ X = [double]$parts[0]; Y = [double]$parts[1] }
}

function Send-Pointer([double]$x, [double]$y) {
    Write-Line ([ordered]@{ viewport = $Viewport; pointer = @($x, $y) })
}

function Send-Button([double]$x, [double]$y, [bool]$pressed) {
    Write-Line ([ordered]@{ viewport = $Viewport; pointer = @($x, $y); button = 'left'; pressed = $pressed })
}

$wrote = 0
if ($Move) {
    $p = Read-Point $Move
    Send-Pointer $p.X $p.Y
    $wrote++
}
if ($Click) {
    $p = Read-Point $Click
    Send-Pointer $p.X $p.Y
    Start-Sleep -Milliseconds $DelayMs
    Send-Button $p.X $p.Y $true
    Start-Sleep -Milliseconds $DelayMs
    Send-Button $p.X $p.Y $false
    $wrote++
}
if ($Drag) {
    $parts = $Drag.Split(',')
    if ($parts.Length -ne 4) { throw "expected x1,y1,x2,y2 but got '$Drag'" }
    $from = Read-Point ($parts[0] + ',' + $parts[1])
    $to = Read-Point ($parts[2] + ',' + $parts[3])
    Send-Pointer $from.X $from.Y
    Start-Sleep -Milliseconds $DelayMs
    Send-Button $from.X $from.Y $true
    Start-Sleep -Milliseconds $DelayMs
    Send-Pointer $to.X $to.Y
    Start-Sleep -Milliseconds $DelayMs
    Send-Button $to.X $to.Y $false
    $wrote++
}
if ($Key) {
    Write-Line ([ordered]@{ viewport = $Viewport; key = $Key; pressed = $true })
    Start-Sleep -Milliseconds $DelayMs
    Write-Line ([ordered]@{ viewport = $Viewport; key = $Key; pressed = $false })
    $wrote++
}
if ($Text) {
    Write-Line ([ordered]@{ viewport = $Viewport; text = $Text })
    $wrote++
}
if ($wrote -eq 0) {
    throw "pass -Move, -Click, -Drag, -Key or -Text"
}
"wrote $wrote command(s) to $File"
