# 生成 assets\jie.ico：新野兽派「取景框」。紫底圆角方块，白色取景框，右下角柠檬黄方块；墨色粗描边、错位实心阴影。
# 造型画在 100 × 100 的网格上，和 scripts/macos/icon.swift、设计稿是同一份坐标；改造型时三处一起改。
# 每个尺寸按比例单独画：48 及以下存 32 位 BMP（老程序也认），256 存 PNG（文件小）。托盘、窗口图标都从这里取。
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Drawing
$ink = [Drawing.Color]::FromArgb(0x15, 0x13, 0x17)
$purple = [Drawing.Color]::FromArgb(0x9C, 0x44, 0xEC)
$lemon = [Drawing.Color]::FromArgb(0xFF, 0xD4, 0x3B)

function RoundRect([float]$x, [float]$y, [float]$w, [float]$h, [float]$r) {
    $p = New-Object Drawing.Drawing2D.GraphicsPath
    $d = 2 * $r
    $p.AddArc($x, $y, $d, $d, 180, 90)
    $p.AddArc($x + $w - $d, $y, $d, $d, 270, 90)
    $p.AddArc($x + $w - $d, $y + $h - $d, $d, $d, 0, 90)
    $p.AddArc($x, $y + $h - $d, $d, $d, 90, 90)
    $p.CloseFigure()
    $p
}

function RoundPen($color, [float]$width) {
    $pen = New-Object Drawing.Pen $color, $width
    $pen.StartCap = [Drawing.Drawing2D.LineCap]::Round
    $pen.EndCap = [Drawing.Drawing2D.LineCap]::Round
    $pen.LineJoin = [Drawing.Drawing2D.LineJoin]::Round
    $pen
}

function Draw([int]$size) {
    $bmp = New-Object Drawing.Bitmap $size, $size, ([Drawing.Imaging.PixelFormat]::Format32bppArgb)
    $g = [Drawing.Graphics]::FromImage($bmp)
    $g.SmoothingMode = [Drawing.Drawing2D.SmoothingMode]::AntiAlias
    $g.PixelOffsetMode = [Drawing.Drawing2D.PixelOffsetMode]::HighQuality
    $g.ScaleTransform($size / 100, $size / 100)
    # 底板：错位 5.5 的墨色阴影，紫底、墨色描边 5
    $g.FillPath((New-Object Drawing.SolidBrush $ink), (RoundRect 10 10 84 84 20))
    $tile = RoundRect 4.5 4.5 83 83 19
    $g.FillPath((New-Object Drawing.SolidBrush $purple), $tile)
    $g.DrawPath((New-Object Drawing.Pen $ink, 5), $tile)
    # 取景框三个角：墨色粗线上叠白线，就是带墨边的白框
    $frame = New-Object Drawing.Drawing2D.GraphicsPath
    foreach ($pts in @(@(25, 45, 25, 25, 45, 25), @(57, 25, 69, 25, 69, 41), @(25, 57, 25, 69, 41, 69))) {
        $frame.StartFigure()
        $frame.AddLines([Drawing.PointF[]]@(
            (New-Object Drawing.PointF $pts[0], $pts[1]),
            (New-Object Drawing.PointF $pts[2], $pts[3]),
            (New-Object Drawing.PointF $pts[4], $pts[5])))
    }
    # 24 像素及以下白线加粗（墨边跟着变窄），不然白线只有一像素多，夹在墨边里发灰
    $g.DrawPath((RoundPen $ink 16), $frame)
    $g.DrawPath((RoundPen ([Drawing.Color]::White) $(if ($size -le 24) { 10.5 } else { 7.5 })), $frame)
    # 右下角柠檬黄方块，错位 5 的墨色阴影
    $g.FillPath((New-Object Drawing.SolidBrush $ink), (RoundRect 57 57 24 24 4))
    $sq = RoundRect 52 52 24 24 4
    $g.FillPath((New-Object Drawing.SolidBrush $lemon), $sq)
    $g.DrawPath((New-Object Drawing.Pen $ink, 4.5), $sq)
    $g.Dispose()
    $bmp
}

$sizes = 16, 20, 24, 32, 48, 256
$images = foreach ($s in $sizes) {
    $bmp = Draw $s
    $ms = New-Object IO.MemoryStream
    if ($s -ge 256) {
        $bmp.Save($ms, [Drawing.Imaging.ImageFormat]::Png)
    } else {
        # BITMAPINFOHEADER（高度写两倍：图像 + AND 掩码）、自下而上的 BGRA（不预乘）、全 0 的 AND 掩码
        $data = $bmp.LockBits((New-Object Drawing.Rectangle 0, 0, $s, $s), [Drawing.Imaging.ImageLockMode]::ReadOnly, [Drawing.Imaging.PixelFormat]::Format32bppArgb)
        $px = New-Object byte[] ($s * $s * 4)
        [Runtime.InteropServices.Marshal]::Copy($data.Scan0, $px, 0, $px.Length)
        $bmp.UnlockBits($data)
        $w = New-Object IO.BinaryWriter $ms
        foreach ($v in 40, $s, (2 * $s)) { $w.Write([int]$v) }
        $w.Write([int16]1); $w.Write([int16]32)
        foreach ($v in 0, $px.Length, 0, 0, 0, 0) { $w.Write([int]$v) }
        for ($y = $s - 1; $y -ge 0; $y--) { $w.Write($px, $y * $s * 4, $s * 4) }
        $w.Write((New-Object byte[] ([math]::Ceiling($s / 32) * 4 * $s)))
        $w.Flush()
    }
    $bmp.Dispose()
    , $ms.ToArray()
}

$ms = New-Object IO.MemoryStream
$w = New-Object IO.BinaryWriter $ms
$w.Write([int16]0); $w.Write([int16]1); $w.Write([int16]$sizes.Count)
$offset = 6 + 16 * $sizes.Count
for ($i = 0; $i -lt $sizes.Count; $i++) {
    $b = if ($sizes[$i] -ge 256) { 0 } else { $sizes[$i] }
    $w.Write([byte]$b); $w.Write([byte]$b); $w.Write([byte]0); $w.Write([byte]0)
    $w.Write([int16]1); $w.Write([int16]32)
    $w.Write([int]$images[$i].Length); $w.Write([int]$offset)
    $offset += $images[$i].Length
}
foreach ($img in $images) { $w.Write([byte[]]$img) }
$w.Flush()
$out = Join-Path $PSScriptRoot '..\..\assets\jie.ico'
[IO.File]::WriteAllBytes((Resolve-Path $out).Path, $ms.ToArray())
"$out  $($ms.Length) bytes"
