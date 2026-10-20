# Generates assets\venu.ico from assets\icon.png, the Venu logo (the same
# transparent cut-out the website uses as its favicon).
#
# The ICO carries one PNG-compressed entry per size Windows asks for — taskbar,
# title bar, tray, Explorer at every DPI — each resampled from the 512px source
# rather than leaving Windows to shrink the 256px one on the fly, which is what
# makes small icons look soft.
#
# Run from the repo root after changing the logo:
#   powershell -File scripts\gen-icon.ps1
Add-Type -AssemblyName System.Drawing

$root = Split-Path $PSScriptRoot -Parent
$source = [System.Drawing.Image]::FromFile((Join-Path $root "assets\icon.png"))
$sizes = 16, 20, 24, 32, 40, 48, 64, 96, 128, 256

$entries = foreach ($size in $sizes) {
    $bmp = New-Object System.Drawing.Bitmap($size, $size, [System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.InterpolationMode = [System.Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic
    $g.PixelOffsetMode = [System.Drawing.Drawing2D.PixelOffsetMode]::HighQuality
    $g.CompositingQuality = [System.Drawing.Drawing2D.CompositingQuality]::HighQuality
    $g.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::HighQuality
    $g.Clear([System.Drawing.Color]::Transparent)
    $g.DrawImage($source, 0, 0, $size, $size)
    $g.Dispose()

    $ms = New-Object System.IO.MemoryStream
    $bmp.Save($ms, [System.Drawing.Imaging.ImageFormat]::Png)
    $bmp.Dispose()
    [pscustomobject]@{ Size = $size; Png = $ms.ToArray() }
}
$source.Dispose()

# ICONDIR (6 bytes), one ICONDIRENTRY (16 bytes) per size, then the payloads.
$out = New-Object System.IO.MemoryStream
$w = New-Object System.IO.BinaryWriter($out)
$w.Write([UInt16]0); $w.Write([UInt16]1); $w.Write([UInt16]$entries.Count)  # reserved, type=icon, count

$offset = 6 + 16 * $entries.Count
foreach ($e in $entries) {
    $dim = if ($e.Size -ge 256) { 0 } else { $e.Size }                   # 256 is written as 0
    $w.Write([byte]$dim); $w.Write([byte]$dim)
    $w.Write([byte]0); $w.Write([byte]0)                                 # colours, reserved
    $w.Write([UInt16]1); $w.Write([UInt16]32)                            # planes, bpp
    $w.Write([UInt32]$e.Png.Length); $w.Write([UInt32]$offset)           # payload size, offset
    $offset += $e.Png.Length
}
foreach ($e in $entries) { $w.Write($e.Png) }
$w.Flush()

$dest = Join-Path $root "assets\venu.ico"
[System.IO.File]::WriteAllBytes($dest, $out.ToArray())
Write-Host ("Wrote {0} ({1} bytes, sizes {2})" -f $dest, $out.Length, ($sizes -join ", "))
