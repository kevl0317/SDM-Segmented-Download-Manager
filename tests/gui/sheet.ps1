# Contact sheets of the captured states, with captions, for reviewing the UI at a glance.
param([string]$Dir, [string]$OutPrefix, [int]$PerSheet = 9, [int]$Columns = 3, [double]$Scale = 0.62)
Add-Type -AssemblyName System.Drawing
$files = Get-ChildItem $Dir -Filter *.png | Sort-Object Name
$sheetNo = 0
for ($i = 0; $i -lt $files.Count; $i += $PerSheet) {
    $batch = $files[$i..([Math]::Min($i + $PerSheet - 1, $files.Count - 1))]
    $cellW = 490; $cellH = 440
    $rows = [Math]::Ceiling($batch.Count / $Columns)
    $sheet = New-Object System.Drawing.Bitmap ($Columns * $cellW), ($rows * $cellH)
    $g = [System.Drawing.Graphics]::FromImage($sheet)
    $g.InterpolationMode = [System.Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic
    $g.Clear([System.Drawing.Color]::FromArgb(70, 70, 70))
    $font = New-Object System.Drawing.Font "Segoe UI", 10, ([System.Drawing.FontStyle]::Bold)
    for ($j = 0; $j -lt $batch.Count; $j++) {
        $img = [System.Drawing.Image]::FromFile($batch[$j].FullName)
        $x = ($j % $Columns) * $cellW + 6
        $y = [Math]::Floor($j / $Columns) * $cellH + 24
        $w = [int]($img.Width * $Scale); $h = [int]($img.Height * $Scale)
        if ($w -gt ($cellW - 12)) { $k = ($cellW - 12) / $w; $w = [int]($w * $k); $h = [int]($h * $k) }
        $g.DrawImage($img, $x, $y, $w, $h)
        $g.DrawString($batch[$j].BaseName, $font, [System.Drawing.Brushes]::White, [float]$x, [float]($y - 20))
        $img.Dispose()
    }
    $g.Dispose()
    $sheetNo++
    $out = "$OutPrefix-$sheetNo.png"
    $sheet.Save($out, [System.Drawing.Imaging.ImageFormat]::Png)
    $sheet.Dispose()
    $out
}
