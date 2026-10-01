// Regenerate macOS app icon assets with: swift scripts/generate-icon.swift
import Foundation
import CoreGraphics
import ImageIO

let root = URL(fileURLWithPath: "crates/desktop/icons")
let iconset = root.appendingPathComponent("Themis.iconset")
try FileManager.default.createDirectory(at: iconset, withIntermediateDirectories: true)
func render(_ size: Int, _ url: URL) {
    let ctx = CGContext(data: nil, width: size, height: size, bitsPerComponent: 8, bytesPerRow: size * 4, space: CGColorSpaceCreateDeviceRGB(), bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!
    let sourceURL = root.appendingPathComponent("icon-source.png")
    guard let source = CGImageSourceCreateWithURL(sourceURL as CFURL, nil),
          let image = CGImageSourceCreateImageAtIndex(source, 0, nil) else {
        fatalError("Missing icon-source.png; rasterize icons/icon.svg at 1024px first")
    }
    ctx.interpolationQuality = .high
    ctx.draw(image, in: CGRect(x: 0, y: 0, width: size, height: size))
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

// Windows accepts a PNG payload in the standard ICO container.
let png = try Data(contentsOf: root.appendingPathComponent("128x128@2x.png"))
func littleEndian(_ value: Int) -> Data { var n = UInt32(value).littleEndian; return Data(bytes: &n, count: 4) }
var ico = Data([0, 0, 1, 0, 1, 0, 0, 0, 0, 0, 1, 0, 32, 0])
ico.append(littleEndian(png.count)); ico.append(littleEndian(22)); ico.append(png)
try ico.write(to: root.appendingPathComponent("icon.ico"))
