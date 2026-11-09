import AppKit

// 新野兽派「取景框」，和 scripts/windows/icon.ps1 同一份 100 × 100 网格上的坐标：白色取景框带墨边，右下角柠檬黄方块带错位实心阴影。
// mac 上紫色底板铺满整个图标、不画底板的描边和阴影（macOS 26 起，不是满版圆角方块的图标会被系统套进灰底方块），框和方块放大 1.15 倍居中。
let output = URL(fileURLWithPath: CommandLine.arguments[1], isDirectory: true)
try FileManager.default.createDirectory(at: output, withIntermediateDirectories: true)
func rgb(_ hex: Int) -> NSColor {
    NSColor(srgbRed: CGFloat(hex >> 16 & 255) / 255, green: CGFloat(hex >> 8 & 255) / 255, blue: CGFloat(hex & 255) / 255, alpha: 1)
}
let ink = rgb(0x151317), purple = rgb(0x9C44EC), lemon = rgb(0xFFD43B)
let corners: [[NSPoint]] = [
    [NSPoint(x: 25, y: 45), NSPoint(x: 25, y: 25), NSPoint(x: 45, y: 25)],
    [NSPoint(x: 57, y: 25), NSPoint(x: 69, y: 25), NSPoint(x: 69, y: 41)],
    [NSPoint(x: 25, y: 57), NSPoint(x: 25, y: 69), NSPoint(x: 41, y: 69)],
]
for size in [16, 32, 128, 256, 512] {
    for scale in [1, 2] {
        let pixels = size * scale
        let rep = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: pixels, pixelsHigh: pixels,
            bitsPerSample: 8, samplesPerPixel: 4, hasAlpha: true, isPlanar: false,
            colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0)!
        let context = NSGraphicsContext(bitmapImageRep: rep)!
        NSGraphicsContext.saveGraphicsState()
        NSGraphicsContext.current = context
        let cg = context.cgContext
        // 换成 100 × 100、y 朝下的网格
        cg.translateBy(x: 0, y: CGFloat(pixels))
        cg.scaleBy(x: CGFloat(pixels) / 100, y: -CGFloat(pixels) / 100)
        purple.setFill()
        NSBezierPath(roundedRect: NSRect(x: 0, y: 0, width: 100, height: 100), xRadius: 22.5, yRadius: 22.5).fill()
        // 框加阴影的外框是 17…81，按它的中心 49 居中
        cg.translateBy(x: 50, y: 50)
        cg.scaleBy(x: 1.15, y: 1.15)
        cg.translateBy(x: -49, y: -49)
        let frame = NSBezierPath()
        frame.lineCapStyle = .round
        frame.lineJoinStyle = .round
        for points in corners {
            frame.move(to: points[0])
            for point in points.dropFirst() { frame.line(to: point) }
        }
        frame.lineWidth = 16
        ink.setStroke()
        frame.stroke()
        // 24 像素及以下白线加粗，不然夹在墨边里发灰（和 Windows 一样）
        frame.lineWidth = pixels <= 24 ? 10.5 : 7.5
        NSColor.white.setStroke()
        frame.stroke()
        ink.setFill()
        NSBezierPath(roundedRect: NSRect(x: 57, y: 57, width: 24, height: 24), xRadius: 4, yRadius: 4).fill()
        let square = NSBezierPath(roundedRect: NSRect(x: 52, y: 52, width: 24, height: 24), xRadius: 4, yRadius: 4)
        lemon.setFill()
        square.fill()
        square.lineWidth = 4.5
        ink.setStroke()
        square.stroke()
        NSGraphicsContext.restoreGraphicsState()
        let suffix = scale == 2 ? "@2x" : ""
        let file = output.appendingPathComponent("icon_\(size)x\(size)\(suffix).png")
        try rep.representation(using: .png, properties: [:])!.write(to: file)
        assert(rep.pixelsWide == pixels && rep.pixelsHigh == pixels)
    }
}
