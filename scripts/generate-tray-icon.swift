// Run on macOS: swift -module-cache-path /tmp/rx-switch-swift-cache \
//   scripts/generate-tray-icon.swift src-tauri/icons/tray-template.png
// Render an SF Symbol as a monochrome Retina asset. The running app marks
// this image as a template so AppKit supplies light/dark/highlight colors.
import AppKit

guard CommandLine.arguments.count == 2,
      let symbol = NSImage(systemSymbolName: "rectangle.connected.to.line.below", accessibilityDescription: nil),
      let configured = symbol.withSymbolConfiguration(
        NSImage.SymbolConfiguration(pointSize: 16, weight: .medium)
          .applying(NSImage.SymbolConfiguration(paletteColors: [.black]))
      ),
      let bitmap = NSBitmapImageRep(
        bitmapDataPlanes: nil, pixelsWide: 40, pixelsHigh: 36,
        bitsPerSample: 8, samplesPerPixel: 4, hasAlpha: true, isPlanar: false,
        colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0
      ),
      let context = NSGraphicsContext(bitmapImageRep: bitmap)
else {
  fatalError("Usage: generate-tray-icon.swift output.png (requires SF Symbols)")
}

NSGraphicsContext.saveGraphicsState()
NSGraphicsContext.current = context
context.shouldAntialias = true
let ratio = min(36 / configured.size.width, 32 / configured.size.height)
let width = configured.size.width * ratio
let height = configured.size.height * ratio
configured.draw(in: NSRect(x: (40 - width) / 2, y: (36 - height) / 2, width: width, height: height))
NSGraphicsContext.restoreGraphicsState()

guard let png = bitmap.representation(using: .png, properties: [:]) else {
  fatalError("Could not encode tray template")
}
try png.write(to: URL(fileURLWithPath: CommandLine.arguments[1]))
