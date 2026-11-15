// Renders the Mercury Browser logo as a macOS application icon.
//
//   swift scripts/macos/render-icon.swift <logo.svg> <size in px> <out.png>
//
// Called by scripts/macos/make-icns.sh once per iconset size. AppKit draws the
// SVG natively (macOS 13 or later), so the artwork stays vector until the final
// size and nothing has to be installed.
//
// Layout follows Apple's macOS icon grid, expressed for a 1024 px canvas and
// scaled to the requested size: a 824 px rounded square centred on the canvas
// (100 px margin, corner radius 185.4 px) with a soft drop shadow, and the logo
// centred on it inside a 600 px box so it does not run to the edges.

import AppKit

let arguments = CommandLine.arguments
guard arguments.count == 4, let size = Int(arguments[2]), size >= 16 else {
    FileHandle.standardError.write("usage: render-icon.swift <logo.svg> <size> <out.png>\n".data(using: .utf8)!)
    exit(2)
}
guard let logo = NSImage(contentsOfFile: arguments[1]) else {
    FileHandle.standardError.write("cannot read \(arguments[1])\n".data(using: .utf8)!)
    exit(1)
}

let scale = CGFloat(size) / 1024
let colorSpace = CGColorSpace(name: CGColorSpace.sRGB)!
guard let context = CGContext(
    data: nil, width: size, height: size, bitsPerComponent: 8, bytesPerRow: 0,
    space: colorSpace, bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)
else { exit(1) }
context.interpolationQuality = .high
context.setShouldAntialias(true)

NSGraphicsContext.saveGraphicsState()
NSGraphicsContext.current = NSGraphicsContext(cgContext: context, flipped: false)

let plate = NSRect(x: 100 * scale, y: 100 * scale, width: 824 * scale, height: 824 * scale)
let platePath = NSBezierPath(roundedRect: plate, xRadius: 185.4 * scale, yRadius: 185.4 * scale)

// Shadow below the plate, as on Apple's own icons.
context.saveGState()
context.setShadow(
    offset: CGSize(width: 0, height: -10 * scale), blur: 20 * scale,
    color: NSColor(white: 0, alpha: 0.3).cgColor)
NSColor.white.setFill()
platePath.fill()
context.restoreGState()

// A light plate, white at the top shading to a pale blue, so the blue logo
// keeps its contrast in both light and dark Dock appearances.
let gradient = NSGradient(
    starting: NSColor(srgbRed: 1, green: 1, blue: 1, alpha: 1),
    ending: NSColor(srgbRed: 0.86, green: 0.91, blue: 0.98, alpha: 1))!
gradient.draw(in: platePath, angle: -90)

let logoBox = NSRect(x: 212 * scale, y: 212 * scale, width: 600 * scale, height: 600 * scale)
logo.draw(in: logoBox, from: .zero, operation: .sourceOver, fraction: 1)

NSGraphicsContext.restoreGraphicsState()

guard let image = context.makeImage() else { exit(1) }
let bitmap = NSBitmapImageRep(cgImage: image)
guard let png = bitmap.representation(using: .png, properties: [:]) else { exit(1) }
do {
    try png.write(to: URL(fileURLWithPath: arguments[3]))
} catch {
    FileHandle.standardError.write("cannot write \(arguments[3]): \(error)\n".data(using: .utf8)!)
    exit(1)
}
