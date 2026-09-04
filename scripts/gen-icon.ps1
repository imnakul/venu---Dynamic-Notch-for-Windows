# Generates assets\venu.ico - the same procedural mark the app draws for its
# window and tray icons (create_app_icon_data in src\main.rs), rasterised at
# 256x256 and packed as a single PNG-compressed ICO entry.
#
# Run once from the repo root:  powershell -File scripts\gen-icon.ps1
Add-Type -AssemblyName System.Drawing

$grid = 32
$scale = 8
$size = $grid * $scale

$bmp = New-Object System.Drawing.Bitmap($size, $size)
$g = [System.Drawing.Graphics]::FromImage($bmp)

$cyan = [System.Drawing.Color]::FromArgb(255, 56, 189, 248)
$silver = [System.Drawing.Color]::FromArgb(255, 230, 230, 230)

for ($y = 0; $y -lt $grid; $y++) {
    for ($x = 0; $x -lt $grid; $x++) {
        $isStroke = (($x -ge 4 -and $x -le 27 -and ($y -eq 4 -or $y -eq 5 -or $y -eq 26 -or $y -eq 27)) -or
                     ($y -ge 4 -and $y -le 27 -and ($x -eq 4 -or $x -eq 5 -or $x -eq 26 -or $x -eq 27)))
        if (-not $isStroke) { continue }

        $isCorner = (($x -le 8 -or $x -ge 23) -and ($y -le 8 -or $y -ge 23))
        $brush = New-Object System.Drawing.SolidBrush($(if ($isCorner) { $cyan } else { $silver }))
        $g.FillRectangle($brush, $x * $scale, $y * $scale, $scale, $scale)
        $brush.Dispose()
    }
}
$g.Dispose()

# Encode the bitmap as PNG and wrap it in the ICO container:
# ICONDIR (6 bytes) + one ICONDIRENTRY (16 bytes) + payload.
$ms = New-Object System.IO.MemoryStream
$bmp.Save($ms, [System.Drawing.Imaging.ImageFormat]::Png)
$png = $ms.ToArray()
$bmp.Dispose()

$out = New-Object System.IO.MemoryStream
$w = New-Object System.IO.BinaryWriter($out)
$w.Write([UInt16]0); $w.Write([UInt16]1); $w.Write([UInt16]1)  # reserved, type=icon, count
$w.Write([byte]0); $w.Write([byte]0)                           # 256x256 is written as 0
$w.Write([byte]0); $w.Write([byte]0)                           # colours, reserved
$w.Write([UInt16]1); $w.Write([UInt16]32)                      # planes, bpp
$w.Write([UInt32]$png.Length); $w.Write([UInt32]22)            # payload size, payload offset
$w.Write($png)
$w.Flush()

$dest = Join-Path (Split-Path $PSScriptRoot -Parent) "assets\venu.ico"
[System.IO.File]::WriteAllBytes($dest, $out.ToArray())
Write-Host ("Wrote {0} ({1} bytes, 256x256 PNG entry)" -f $dest, $out.Length)
