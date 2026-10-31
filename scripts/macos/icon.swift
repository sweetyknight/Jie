import AppKit

// The same purple selection frame as the Windows icon, drawn at each macOS icon size.
let output = URL(fileURLWithPath: CommandLine.arguments[1], isDirectory: true)
try FileManager.default.createDirectory(at: output, withIntermediateDirectories: true)
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
        cg.scaleBy(x: CGFloat(pixels) / 1024, y: CGFloat(pixels) / 1024)
        let background = NSBezierPath(roundedRect: NSRect(x: 0, y: 0, width: 1024, height: 1024), xRadius: 220, yRadius: 220)
        NSGradient(starting: NSColor(srgbRed: 0.69, green: 0.35, blue: 1, alpha: 1),
            ending: NSColor(srgbRed: 0.48, green: 0.17, blue: 0.86, alpha: 1))!.draw(in: background, angle: -90)
        let frame = NSBezierPath()
        frame.lineWidth = 66
        frame.lineCapStyle = .round
        frame.lineJoinStyle = .round
        for points in [[NSPoint(x: 250, y: 550), NSPoint(x: 250, y: 774), NSPoint(x: 474, y: 774)],
                       [NSPoint(x: 600, y: 774), NSPoint(x: 774, y: 774), NSPoint(x: 774, y: 500)],
                       [NSPoint(x: 250, y: 424), NSPoint(x: 250, y: 250), NSPoint(x: 524, y: 250)]] {
            frame.move(to: points[0])
            for point in points.dropFirst() { frame.line(to: point) }
        }
        NSColor.white.setStroke()
        frame.stroke()
        NSColor.white.setFill()
        NSBezierPath(roundedRect: NSRect(x: 650, y: 194, width: 180, height: 180), xRadius: 24, yRadius: 24).fill()
        NSGraphicsContext.restoreGraphicsState()
        let suffix = scale == 2 ? "@2x" : ""
        let file = output.appendingPathComponent("icon_\(size)x\(size)\(suffix).png")
        try rep.representation(using: .png, properties: [:])!.write(to: file)
        assert(rep.pixelsWide == pixels && rep.pixelsHigh == pixels)
    }
}
