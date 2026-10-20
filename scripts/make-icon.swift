import AppKit

// Renders the Loupe app icon: a magnifier over diff lines, on the macOS 1024 grid (824 body, soft shadow).
// Run: swift scripts/make-icon.swift [preview.png]   — without an argument writes scripts/AppIcon.icns.

func srgb(_ hex: UInt32, _ a: CGFloat = 1) -> CGColor {
    CGColor(srgbRed: CGFloat((hex >> 16) & 0xff) / 255, green: CGFloat((hex >> 8) & 0xff) / 255, blue: CGFloat(hex & 0xff) / 255, alpha: a)
}

func linear(_ ctx: CGContext, _ colors: [CGColor], from: CGPoint, to: CGPoint) {
    let g = CGGradient(colorsSpace: CGColorSpace(name: CGColorSpace.sRGB), colors: colors as CFArray, locations: nil)!
    ctx.drawLinearGradient(g, start: from, end: to, options: [.drawsBeforeStartLocation, .drawsAfterEndLocation])
}

/// Diff "code" bars: (indent, length, kind) with kind 0 = context, 1 = added, 2 = removed.
let bars: [(CGFloat, CGFloat, Int)] = [
    (0, 0.46, 0), (0.07, 0.62, 0), (0.07, 0.38, 0), (0.07, 0.72, 2), (0.07, 0.58, 2),
    (0.07, 0.66, 1), (0.07, 0.50, 1), (0.14, 0.44, 0), (0.07, 0.30, 0), (0, 0.22, 0),
]

func drawBars(_ ctx: CGContext, in r: CGRect, bright: Bool) {
    let rowH = r.height / CGFloat(bars.count)
    let barH = rowH * (bright ? 0.40 : 0.34)
    for (i, b) in bars.enumerated() {
        // Rows run top → bottom (CG origin is bottom-left).
        let y = r.maxY - rowH * (CGFloat(i) + 0.5) - barH / 2
        let x = r.minX + r.width * b.0
        let w = r.width * b.1
        if b.2 != 0, bright {   // tinted diff rows only inside the lens
            // Full-width tinted row behind added/removed lines, like a real diff.
            ctx.setFillColor(b.2 == 1 ? srgb(0x2EA043, bright ? 0.34 : 0.22) : srgb(0xF85149, bright ? 0.34 : 0.22))
            ctx.fill(CGRect(x: r.minX - r.width * 0.06, y: r.maxY - rowH * CGFloat(i + 1), width: r.width * 1.12, height: rowH))
        }
        // Outside the lens everything is neutral; colour is what the loupe reveals.
        let color: CGColor = !bright ? srgb(0xC9D1D9, 0.28) : b.2 == 1 ? srgb(0x56D364) : b.2 == 2 ? srgb(0xFF7B72) : srgb(0xC9D1D9, 0.9)
        ctx.setFillColor(color)
        ctx.addPath(CGPath(roundedRect: CGRect(x: x, y: y, width: w, height: barH), cornerWidth: barH / 2, cornerHeight: barH / 2, transform: nil))
        ctx.fillPath()
    }
}

func render(_ px: Int) -> Data {
    let s = CGFloat(px)
    let k = s / 1024
    let ctx = CGContext(data: nil, width: px, height: px, bitsPerComponent: 8, bytesPerRow: 0,
                        space: CGColorSpace(name: CGColorSpace.sRGB)!, bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!
    ctx.interpolationQuality = .high

    // Body: 824 × 824 squircle centred on the 1024 canvas.
    let body = CGRect(x: 100 * k, y: 100 * k, width: 824 * k, height: 824 * k)
    let bodyPath = CGPath(roundedRect: body, cornerWidth: 185 * k, cornerHeight: 185 * k, transform: nil)

    ctx.saveGState()
    ctx.setShadow(offset: CGSize(width: 0, height: -10 * k), blur: 28 * k, color: srgb(0x000000, 0.45))
    ctx.addPath(bodyPath); ctx.setFillColor(srgb(0x161821)); ctx.fillPath()
    ctx.restoreGState()

    ctx.saveGState()
    ctx.addPath(bodyPath); ctx.clip()
    linear(ctx, [srgb(0x2B3050), srgb(0x11131C)], from: CGPoint(x: 0, y: body.maxY), to: CGPoint(x: 0, y: body.minY))
    // Subtle top sheen.
    linear(ctx, [srgb(0xFFFFFF, 0.10), srgb(0xFFFFFF, 0)], from: CGPoint(x: 0, y: body.maxY), to: CGPoint(x: 0, y: body.midY + 120 * k))

    // Code area behind the lens.
    let code = CGRect(x: body.minX + 104 * k, y: body.minY + 150 * k, width: 620 * k, height: 560 * k)
    drawBars(ctx, in: code, bright: false)

    // Magnifier.
    let center = CGPoint(x: body.minX + 356 * k, y: body.minY + 468 * k)
    let lensR: CGFloat = 186 * k
    let ring: CGFloat = 40 * k

    // Handle first (under the ring), angled to the bottom-right.
    let angle: CGFloat = -.pi / 4
    let hStart = CGPoint(x: center.x + cos(angle) * (lensR + ring * 0.2), y: center.y + sin(angle) * (lensR + ring * 0.2))
    let hEnd = CGPoint(x: center.x + cos(angle) * (lensR + 236 * k), y: center.y + sin(angle) * (lensR + 236 * k))
    ctx.saveGState()
    ctx.setShadow(offset: CGSize(width: 0, height: -8 * k), blur: 18 * k, color: srgb(0x000000, 0.5))
    ctx.setLineCap(.round)
    ctx.setLineWidth(70 * k)
    ctx.setStrokeColor(srgb(0x4A5163))
    ctx.move(to: hStart); ctx.addLine(to: hEnd); ctx.strokePath()
    ctx.restoreGState()
    // Handle shading.
    ctx.saveGState()
    ctx.setLineCap(.round); ctx.setLineWidth(70 * k)
    ctx.move(to: hStart); ctx.addLine(to: hEnd); ctx.replacePathWithStrokedPath(); ctx.clip()
    linear(ctx, [srgb(0x9AA3B5), srgb(0x5B6375), srgb(0x323846)], from: CGPoint(x: hStart.x - 40 * k, y: hStart.y + 40 * k), to: CGPoint(x: hStart.x + 40 * k, y: hStart.y - 40 * k))
    ctx.restoreGState()

    // Lens glass: magnified, brighter copy of the code under it.
    let lens = CGRect(x: center.x - lensR, y: center.y - lensR, width: lensR * 2, height: lensR * 2)
    ctx.saveGState()
    ctx.addEllipse(in: lens); ctx.clip()
    ctx.setFillColor(srgb(0x1A1E2B)); ctx.fill(lens)
    // A legible mini diff: (marker, length as fraction of lens, kind).
    let rows: [(CGFloat, Int)] = [(0.50, 0), (0.62, 2), (0.44, 2), (0.70, 1), (0.54, 1), (0.36, 0)]
    let rowH = lensR * 2 / 6.6
    let barH = rowH * 0.36
    let top = center.y + rowH * 3
    for (i, r) in rows.enumerated() {
        let y0 = top - rowH * CGFloat(i + 1)
        if r.1 != 0 {
            ctx.setFillColor(r.1 == 1 ? srgb(0x2EA043, 0.26) : srgb(0xF85149, 0.26))
            ctx.fill(CGRect(x: lens.minX, y: y0, width: lens.width, height: rowH))
        }
        let markerX = lens.minX + lensR * 0.30
        let barX = markerX + rowH * 0.62
        let midY = y0 + rowH / 2
        if r.1 != 0 {   // + / − in the gutter
            ctx.setStrokeColor(r.1 == 1 ? srgb(0x56D364) : srgb(0xFF7B72))
            ctx.setLineWidth(barH * 0.55); ctx.setLineCap(.round)
            let m = barH * 0.85
            ctx.move(to: CGPoint(x: markerX - m, y: midY)); ctx.addLine(to: CGPoint(x: markerX + m, y: midY))
            if r.1 == 1 { ctx.move(to: CGPoint(x: markerX, y: midY - m)); ctx.addLine(to: CGPoint(x: markerX, y: midY + m)) }
            ctx.strokePath()
        }
        ctx.setFillColor(r.1 == 1 ? srgb(0x7EE787) : r.1 == 2 ? srgb(0xFFA198) : srgb(0xC9D1D9, 0.75))
        ctx.addPath(CGPath(roundedRect: CGRect(x: barX, y: midY - barH / 2, width: lensR * 2 * r.0, height: barH),
                           cornerWidth: barH / 2, cornerHeight: barH / 2, transform: nil))
        ctx.fillPath()
    }
    ctx.restoreGState()
    // Glass highlight + inner vignette.
    ctx.saveGState()
    ctx.addEllipse(in: lens); ctx.clip()
    let g = CGGradient(colorsSpace: CGColorSpace(name: CGColorSpace.sRGB), colors: [srgb(0xFFFFFF, 0.0), srgb(0x000000, 0.35)] as CFArray, locations: [0.7, 1])!
    ctx.drawRadialGradient(g, startCenter: center, startRadius: 0, endCenter: center, endRadius: lensR, options: [])
    let hl = CGRect(x: center.x - lensR * 0.78, y: center.y + lensR * 0.12, width: lensR * 1.1, height: lensR * 0.75)
    ctx.addEllipse(in: hl); ctx.clip()
    linear(ctx, [srgb(0xFFFFFF, 0.28), srgb(0xFFFFFF, 0)], from: CGPoint(x: 0, y: hl.maxY), to: CGPoint(x: 0, y: hl.minY))
    ctx.restoreGState()

    // Ring: brushed-metal gradient stroke.
    ctx.saveGState()
    ctx.setShadow(offset: CGSize(width: 0, height: -6 * k), blur: 14 * k, color: srgb(0x000000, 0.45))
    ctx.setLineWidth(ring); ctx.setStrokeColor(srgb(0xD8DEE6))
    ctx.strokeEllipse(in: lens.insetBy(dx: -ring / 2, dy: -ring / 2))
    ctx.restoreGState()
    ctx.saveGState()
    ctx.setLineWidth(ring)
    ctx.addEllipse(in: lens.insetBy(dx: -ring / 2, dy: -ring / 2)); ctx.replacePathWithStrokedPath(); ctx.clip()
    linear(ctx, [srgb(0xFFFFFF), srgb(0xC5CCD6), srgb(0x8B95A3), srgb(0xE6EAF0)],
           from: CGPoint(x: lens.minX, y: lens.maxY + ring), to: CGPoint(x: lens.maxX, y: lens.minY - ring))
    ctx.restoreGState()

    ctx.restoreGState()
    // Hairline edge so the icon holds on light and dark docks.
    ctx.addPath(bodyPath); ctx.setStrokeColor(srgb(0xFFFFFF, 0.08)); ctx.setLineWidth(2 * k); ctx.strokePath()

    let rep = NSBitmapImageRep(cgImage: ctx.makeImage()!)
    return rep.representation(using: .png, properties: [:])!
}

let dir = URL(fileURLWithPath: CommandLine.arguments[0]).deletingLastPathComponent()
if CommandLine.arguments.count > 1 {
    try! render(1024).write(to: URL(fileURLWithPath: CommandLine.arguments[1]))
} else {
    let set = dir.appendingPathComponent("AppIcon.iconset")
    try? FileManager.default.removeItem(at: set)
    try! FileManager.default.createDirectory(at: set, withIntermediateDirectories: true)
    for (name, px) in [("16x16", 16), ("16x16@2x", 32), ("32x32", 32), ("32x32@2x", 64), ("128x128", 128), ("128x128@2x", 256),
                       ("256x256", 256), ("256x256@2x", 512), ("512x512", 512), ("512x512@2x", 1024)] {
        try! render(px).write(to: set.appendingPathComponent("icon_\(name).png"))
    }
}
