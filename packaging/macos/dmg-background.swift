// Draws the disk image's window background: Voxferro's greys, an arrow
// from where the app sits to where Applications sits, and a line saying
// what to do. Writes background.png (640 x 400) and background@2x.png.
//
//   swift packaging/macos/dmg-background.swift packaging/macos
import AppKit

let out = CommandLine.arguments.count > 1 ? CommandLine.arguments[1] : "."
let size = NSSize(width: 640, height: 400)

func draw(scale: CGFloat) -> Data {
    let rep = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: Int(size.width * scale), pixelsHigh: Int(size.height * scale),
                               bitsPerSample: 8, samplesPerPixel: 4, hasAlpha: true, isPlanar: false,
                               colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0)!
    rep.size = size
    NSGraphicsContext.saveGraphicsState()
    NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: rep)

    // The app's panel grey, a little lighter toward the top.
    let gradient = NSGradient(starting: NSColor(calibratedRed: 0.20, green: 0.20, blue: 0.21, alpha: 1),
                              ending: NSColor(calibratedRed: 0.13, green: 0.13, blue: 0.14, alpha: 1))!
    gradient.draw(in: NSRect(origin: .zero, size: size), angle: -90)

    // The arrow, between the two icons (centred at x 170 and 470, y 190
    // from the top -- 210 from the bottom in AppKit's coordinates).
    let y: CGFloat = 210
    let blue = NSColor(calibratedRed: 0.43, green: 0.67, blue: 0.94, alpha: 1)
    blue.setStroke()
    blue.setFill()
    let shaft = NSBezierPath()
    shaft.move(to: NSPoint(x: 258, y: y))
    shaft.line(to: NSPoint(x: 366, y: y))
    shaft.lineWidth = 6
    shaft.lineCapStyle = .round
    shaft.stroke()
    let head = NSBezierPath()
    head.move(to: NSPoint(x: 386, y: y))
    head.line(to: NSPoint(x: 360, y: y + 17))
    head.line(to: NSPoint(x: 360, y: y - 17))
    head.close()
    head.fill()

    // The title and the instruction.
    let centred = NSMutableParagraphStyle()
    centred.alignment = .center
    let title: [NSAttributedString.Key: Any] = [
        .font: NSFont.systemFont(ofSize: 26, weight: .semibold),
        .foregroundColor: NSColor(calibratedWhite: 0.92, alpha: 1),
        .paragraphStyle: centred,
    ]
    let line: [NSAttributedString.Key: Any] = [
        .font: NSFont.systemFont(ofSize: 14, weight: .regular),
        .foregroundColor: NSColor(calibratedWhite: 0.62, alpha: 1),
        .paragraphStyle: centred,
    ]
    NSAttributedString(string: "Voxferro", attributes: title).draw(in: NSRect(x: 0, y: 330, width: size.width, height: 36))
    NSAttributedString(string: "Drag Voxferro to Applications to install it", attributes: line)
        .draw(in: NSRect(x: 0, y: 70, width: size.width, height: 22))

    NSGraphicsContext.restoreGraphicsState()
    return rep.representation(using: .png, properties: [:])!
}

try! draw(scale: 1).write(to: URL(fileURLWithPath: "\(out)/background.png"))
try! draw(scale: 2).write(to: URL(fileURLWithPath: "\(out)/background@2x.png"))
print("wrote \(out)/background.png and background@2x.png")
