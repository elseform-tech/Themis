// Regenerate macOS app icon assets with: swift scripts/generate-icon.swift
import Foundation
import CoreGraphics
import ImageIO

let root = URL(fileURLWithPath: "crates/desktop/icons")
let iconset = root.appendingPathComponent("Themis.iconset")
try FileManager.default.createDirectory(at: iconset, withIntermediateDirectories: true)
func render(_ size: Int, _ url: URL) {
    let ctx = CGContext(data: nil, width: size, height: size, bitsPerComponent: 8, bytesPerRow: size * 4, space: CGColorSpaceCreateDeviceRGB(), bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!
    ctx.scaleBy(x: CGFloat(size) / 32, y: CGFloat(size) / 32)
    ctx.setFillColor(CGColor(red: 0.055, green: 0.07, blue: 0.10, alpha: 1))
    ctx.addPath(CGPath(roundedRect: CGRect(x: 1, y: 1, width: 30, height: 30), cornerWidth: 7, cornerHeight: 7, transform: nil)); ctx.fillPath()
    ctx.setStrokeColor(CGColor(red: 0.88, green: 0.72, blue: 0.36, alpha: 1))
    ctx.setLineWidth(1.25); ctx.setLineCap(.round); ctx.setLineJoin(.round)
    func line(_ points: [CGPoint]) { ctx.beginPath(); ctx.addLines(between: points); ctx.strokePath() }
    line([CGPoint(x: 16, y: 7), CGPoint(x: 16, y: 25)])
    line([CGPoint(x: 11, y: 7), CGPoint(x: 21, y: 7)])
    line([CGPoint(x: 7, y: 22), CGPoint(x: 25, y: 22)])
    for x: CGFloat in [8, 24] {
        line([CGPoint(x: x, y: 22), CGPoint(x: x - 4, y: 14), CGPoint(x: x + 4, y: 14), CGPoint(x: x, y: 22)])
        ctx.beginPath(); ctx.move(to: CGPoint(x: x - 4, y: 14)); ctx.addCurve(to: CGPoint(x: x + 4, y: 14), control1: CGPoint(x: x - 3, y: 10), control2: CGPoint(x: x + 3, y: 10)); ctx.strokePath()
    }
    ctx.setFillColor(CGColor(red: 0.88, green: 0.72, blue: 0.36, alpha: 1)); ctx.fillEllipse(in: CGRect(x: 14.5, y: 23.5, width: 3, height: 3))
    let destination = CGImageDestinationCreateWithURL(url as CFURL, "public.png" as CFString, 1, nil)!
    CGImageDestinationAddImage(destination, ctx.makeImage()!, nil)
    precondition(CGImageDestinationFinalize(destination))
}
for base in [16, 32, 128, 256, 512] {
    render(base, iconset.appendingPathComponent("icon_\(base)x\(base).png"))
    render(base * 2, iconset.appendingPathComponent("icon_\(base)x\(base)@2x.png"))
}
for size in [32, 64, 128] { render(size, root.appendingPathComponent("\(size)x\(size).png")) }
render(256, root.appendingPathComponent("128x128@2x.png"))
render(1024, root.appendingPathComponent("icon.png"))

func bigEndian(_ value: Int) -> Data { var n = UInt32(value).bigEndian; return Data(bytes: &n, count: 4) }
var chunks = Data()
for (kind, name) in [("ic07", "icon_128x128.png"), ("ic08", "icon_256x256.png"), ("ic09", "icon_512x512.png"), ("ic10", "icon_512x512@2x.png"), ("ic11", "icon_16x16@2x.png"), ("ic12", "icon_32x32@2x.png")] {
    let png = try Data(contentsOf: iconset.appendingPathComponent(name))
    chunks.append(kind.data(using: .ascii)!); chunks.append(bigEndian(png.count + 8)); chunks.append(png)
}
var icns = "icns".data(using: .ascii)!; icns.append(bigEndian(chunks.count + 8)); icns.append(chunks)
try icns.write(to: root.appendingPathComponent("icon.icns"))
