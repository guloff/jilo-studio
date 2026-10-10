import AppKit
// usage: draw.swift mark.json out.png size [plate:1|0]
let a = CommandLine.arguments
let spec = try! JSONSerialization.jsonObject(with: Data(contentsOf: URL(fileURLWithPath: a[1]))) as! [String: Any]
// a[4]: "1" plate on the 1024 grid (default), "0" no plate, "crop" the plate cut out with a small margin
let size = Int(a[3])!; let mode = a.count < 5 ? "1" : a[4]; let withPlate = mode != "0"
let view: CGFloat = mode == "crop" ? 840 : 1024
let s = CGFloat(size) / view
func col(_ h: String) -> CGColor { let v = Int(h.dropFirst(), radix: 16)!; return CGColor(srgbRed: CGFloat((v>>16)&255)/255, green: CGFloat((v>>8)&255)/255, blue: CGFloat(v&255)/255, alpha: 1) }
let cs = CGColorSpace(name: CGColorSpace.sRGB)!
let ctx = CGContext(data: nil, width: size, height: size, bitsPerComponent: 8, bytesPerRow: 0, space: cs, bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!
ctx.translateBy(x: 0, y: CGFloat(size)); ctx.scaleBy(x: s, y: -s)
if mode == "crop" { ctx.translateBy(x: -92, y: -92) }
ctx.setShouldAntialias(true)
let plate = spec["plate"] as! [String: Any]
let pc = col(plate["color"] as! String)
let disc = spec["disc"] as! [String: Double]
let (cx, cy, R) = (disc["cx"]!, disc["cy"]!, disc["R"]!)
if withPlate {
  let r = CGRect(x: plate["x"] as! Double, y: plate["y"] as! Double, width: plate["w"] as! Double, height: plate["h"] as! Double)
  ctx.addPath(CGPath(roundedRect: r, cornerWidth: plate["r"] as! Double, cornerHeight: plate["r"] as! Double, transform: nil)); ctx.setFillColor(pc); ctx.fillPath()
}
// the mark in its own layer, so the gaps cut through to the plate (or to transparency without one)
ctx.saveGState()
ctx.beginTransparencyLayer(auxiliaryInfo: nil)
ctx.addEllipse(in: CGRect(x: cx-R, y: cy-R, width: 2*R, height: 2*R)); ctx.clip()
let g = spec["grad"] as! [String: Any]
let grad = CGGradient(colorsSpace: cs, colors: [col(g["c1"] as! String), col(g["c2"] as! String)] as CFArray, locations: [0, 1])!
ctx.drawLinearGradient(grad, start: CGPoint(x: g["x1"] as! Double, y: g["y1"] as! Double), end: CGPoint(x: g["x2"] as! Double, y: g["y2"] as! Double), options: [])
ctx.setBlendMode(.clear)
ctx.setLineWidth(spec["gap"] as! Double); ctx.setLineCap(.round); ctx.setLineJoin(.round)
for c in spec["edges"] as! [[[Double]]] {
  ctx.addLines(between: c.map { CGPoint(x: $0[0], y: $0[1]) })
  ctx.strokePath()
}
let o = (spec["opening"] as! [[Double]]).map { CGPoint(x: $0[0], y: $0[1]) }
ctx.addLines(between: o); ctx.closePath(); ctx.fillPath()
ctx.endTransparencyLayer()
ctx.restoreGState()
let img = ctx.makeImage()!
let rep = NSBitmapImageRep(cgImage: img)
try! rep.representation(using: .png, properties: [:])!.write(to: URL(fileURLWithPath: a[2]))
