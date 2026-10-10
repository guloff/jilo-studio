import AppKit
// usage: bg.swift bg.json out.png scale   (660 x 400 pt; scale 1 or 2)
let a = CommandLine.arguments
let j = try! JSONSerialization.jsonObject(with: Data(contentsOf: URL(fileURLWithPath: a[1]))) as! [String: Any]
let sc = CGFloat(Double(a[3])!)
let W = Int(660*sc), H = Int(400*sc)
func col(_ h: String, _ al: CGFloat = 1) -> CGColor { let v = Int(h.dropFirst(), radix: 16)!; return CGColor(srgbRed: CGFloat((v>>16)&255)/255, green: CGFloat((v>>8)&255)/255, blue: CGFloat(v&255)/255, alpha: al) }
let cs = CGColorSpace(name: CGColorSpace.sRGB)!
let c = CGContext(data: nil, width: W, height: H, bitsPerComponent: 8, bytesPerRow: 0, space: cs, bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!
c.translateBy(x: 0, y: CGFloat(H)); c.scaleBy(x: sc, y: -sc)
let paper = col(j["paper"] as! String), ink = col("#15131D"), plate = col(j["plate"] as! String)
let c1 = col(j["c1"] as! String), c2 = col(j["c2"] as! String)
c.setFillColor(paper); c.fill(CGRect(x: 0, y: 0, width: 660, height: 400))
c.saveGState(); c.clip(to: CGRect(x: 0, y: 0, width: 232, height: 400))
c.setFillColor(plate); c.fill(CGRect(x: 0, y: 0, width: 232, height: 400))
let cx = j["cx"] as! Double, cy = j["cy"] as! Double, R = j["R"] as! Double, gap = j["gap"] as! Double
c.addEllipse(in: CGRect(x: cx-R, y: cy-R, width: 2*R, height: 2*R)); c.clip()
let g = CGGradient(colorsSpace: cs, colors: [c1, c2] as CFArray, locations: [0, 1])!
c.drawLinearGradient(g, start: CGPoint(x: cx-R, y: cy-R), end: CGPoint(x: cx+R, y: cy+R), options: [])
c.setStrokeColor(plate); c.setLineWidth(gap); c.setLineCap(.round); c.setLineJoin(.round)
for e in j["edges"] as! [[[Double]]] { c.addLines(between: e.map { CGPoint(x: $0[0], y: $0[1]) }); c.strokePath() }
c.setFillColor(plate); c.addLines(between: (j["opening"] as! [[Double]]).map { CGPoint(x: $0[0], y: $0[1]) }); c.closePath(); c.fillPath()
c.restoreGState()
func line(_ x1: Double, _ y1: Double, _ x2: Double, _ y2: Double, _ w: Double) { c.setStrokeColor(ink); c.setLineWidth(w); c.setLineCap(.butt); c.move(to: CGPoint(x: x1, y: y1)); c.addLine(to: CGPoint(x: x2, y: y2)); c.strokePath() }
func rr(_ x: Double, _ y: Double, _ w: Double, _ h: Double, _ r: Double, _ fill: CGColor, stroke: Double = 0) {
  let p = CGPath(roundedRect: CGRect(x: x, y: y, width: w, height: h), cornerWidth: r, cornerHeight: r, transform: nil)
  c.addPath(p); c.setFillColor(fill); c.fillPath()
  if stroke > 0 { c.addPath(p); c.setStrokeColor(ink); c.setLineWidth(stroke); c.strokePath() }
}
line(232.5, 0, 232.5, 400, 1.2); line(236.5, 0, 236.5, 400, 0.5)
line(267, 88.5, 624, 88.5, 0.6); rr(267, 84, 22, 9, 3, c2, stroke: 0.8)
rr(404, 199, 22, 12, 3.5, ink); rr(430, 199, 12, 12, 3.5, col("#15131D", 0.55)); rr(446, 199, 12, 12, 3.5, col("#15131D", 0.25)); rr(427.2, 194, 1.6, 22, 0.8, ink)
c.move(to: CGPoint(x: 466, y: 192)); c.addLine(to: CGPoint(x: 490, y: 205)); c.addLine(to: CGPoint(x: 466, y: 218)); c.closePath()
c.setFillColor(c2); c.setStrokeColor(ink); c.setLineWidth(1.2); c.setLineJoin(.round); c.drawPath(using: .fillStroke)
line(267, 335.5, 624, 335.5, 0.6)
let rep = NSBitmapImageRep(cgImage: c.makeImage()!)
try! rep.representation(using: .png, properties: [:])!.write(to: URL(fileURLWithPath: a[2]))
