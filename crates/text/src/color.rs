//! Colour glyphs: text set in a colour font with `COLR` / `CPAL` tables (v0 layers and v1 paint
//! graphs: solid fills, linear, radial and sweep gradients, transforms, clips and layers) drawn
//! into premultiplied RGBA with Jilo Studio's own rasteriser ([`crate::raster`]). Emoji stickers
//! use it on Windows, whose emoji font (Segoe UI Emoji) is a COLR v1 font.
//!
//! Colours are blended in sRGB, as browsers and the system draw colour fonts. Blend modes other
//! than the Porter-Duff ones, Multiply and Screen are drawn as source-over.

use skrifa::color::{Brush, ColorPainter, ColorStop, CompositeMode, Extend, Transform};
use skrifa::instance::{LocationRef, Size};
use skrifa::outline::{DrawSettings, OutlinePen};
use skrifa::raw::types::BoundingBox;
use skrifa::{FontRef, GlyphId, MetadataProvider};

use crate::fonts;
use crate::layout::{ParagraphStyle, TextStyle, layout};
use crate::raster::{IDENTITY, Mask, Path, Xform, apply, compose, fill_into};

/// Whether `family` is an installed colour (`COLR`) font.
pub fn is_color_family(family: &str) -> bool {
    use skrifa::raw::TableProvider;
    let r = fonts::resolve(family, "Regular");
    !r.missing && fonts::face(r.face).font().is_some_and(|f| f.colr().is_ok())
}

/// `text` set in the colour font `family`, `size` pixels to the em: (width, height, premultiplied
/// RGBA rows). None when none of the glyphs has colour.
pub fn render(text: &str, family: &str, size: f32) -> Option<(u32, u32, Vec<u8>)> {
    let style = TextStyle { family: family.to_string(), size, ..TextStyle::default() };
    let laid = layout(text, &style, &ParagraphStyle::default());
    let line = laid.lines.first()?;
    let margin = (size * 0.15).ceil();
    let (x0, y0) = (laid.bounds[0].min(line.x) - margin, line.baseline - line.ascent.max(size) - margin);
    let w = ((laid.bounds[2].max(line.x + line.width) - x0 + margin).ceil() as usize).clamp(1, 4096);
    let h = ((line.baseline + line.descent.max(size * 0.3) - y0 + margin).ceil() as usize).clamp(1, 4096);
    let mut canvas = Canvas::new(w, h);
    let mut any = false;
    for g in &laid.glyphs {
        let face = fonts::face(g.face);
        let Some(font) = face.font() else { continue };
        let Some(glyph) = font.color_glyphs().get(GlyphId::new(g.id)) else { continue };
        let k = g.size / face.units_per_em();
        canvas.palette = font
            .color_palettes()
            .get(0)
            .map(|p| p.colors().iter().map(|c| [c.red, c.green, c.blue, c.alpha].map(|v| f32::from(v) / 255.0)).collect())
            .unwrap_or_default();
        // font units (y up) → canvas pixels (y down)
        canvas.xf = vec![[k, 0.0, 0.0, -k, g.x - x0, g.y - y0]];
        canvas.clips.clear();
        canvas.layers.truncate(1);
        if glyph.paint(LocationRef::default(), &mut Painter { canvas: &mut canvas, font: font.clone() }).is_ok() {
            any = true;
        }
    }
    if !any {
        return None;
    }
    let px = canvas.layers.first()?.pixels.iter().flat_map(|p| p.map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8)).collect();
    Some((w as u32, h as u32, px))
}

/// A layer being drawn: premultiplied RGBA, and how it merges into the one below.
struct Layer {
    pixels: Vec<[f32; 4]>,
    mode: CompositeMode,
}

/// A clip: coverage and the pixel box it is non-zero in.
struct Clip {
    mask: Mask,
    bbox: (usize, usize, usize, usize),
}

/// What is drawn: the layers, and the state of the glyph being painted.
struct Canvas {
    w: usize,
    h: usize,
    /// The font's first palette, straight RGBA 0–1.
    palette: Vec<[f32; 4]>,
    xf: Vec<Xform>,
    clips: Vec<Clip>,
    layers: Vec<Layer>,
}

/// Paints one colour glyph of `font` onto the canvas.
struct Painter<'c, 'f> {
    canvas: &'c mut Canvas,
    font: FontRef<'f>,
}

/// Records an outline as a path through a transform.
struct Pen<'p> {
    path: &'p mut Path,
    m: Xform,
}

impl OutlinePen for Pen<'_> {
    fn move_to(&mut self, x: f32, y: f32) {
        let (a, b) = apply(&self.m, x, y);
        self.path.move_to(a, b);
    }
    fn line_to(&mut self, x: f32, y: f32) {
        let (a, b) = apply(&self.m, x, y);
        self.path.line_to(a, b);
    }
    fn quad_to(&mut self, cx: f32, cy: f32, x: f32, y: f32) {
        let ((c0, c1), (a, b)) = (apply(&self.m, cx, cy), apply(&self.m, x, y));
        self.path.quad_to(c0, c1, a, b);
    }
    fn curve_to(&mut self, c0x: f32, c0y: f32, c1x: f32, c1y: f32, x: f32, y: f32) {
        let ((p, q), (r, s), (a, b)) = (apply(&self.m, c0x, c0y), apply(&self.m, c1x, c1y), apply(&self.m, x, y));
        self.path.cubic_to(p, q, r, s, a, b);
    }
    fn close(&mut self) {
        self.path.close();
    }
}

fn xform_of(t: &Transform) -> Xform {
    [t.xx, t.yx, t.xy, t.yy, t.dx, t.dy]
}

fn invert(m: &Xform) -> Option<Xform> {
    let det = m[0] * m[3] - m[1] * m[2];
    if det.abs() < 1e-12 || !det.is_finite() {
        return None;
    }
    let (a, b, c, d) = (m[3] / det, -m[1] / det, -m[2] / det, m[0] / det);
    Some([a, b, c, d, -(a * m[4] + c * m[5]), -(b * m[4] + d * m[5])])
}

impl Canvas {
    fn new(w: usize, h: usize) -> Self {
        Self {
            w,
            h,
            palette: Vec::new(),
            xf: Vec::new(),
            clips: Vec::new(),
            layers: vec![Layer { pixels: vec![[0.0; 4]; w * h], mode: CompositeMode::SrcOver }],
        }
    }

    fn current(&self) -> Xform {
        self.xf.last().copied().unwrap_or(IDENTITY)
    }

    /// Push the coverage of `path` (in canvas pixels) within the current clip.
    fn push_clip_path(&mut self, path: Path) {
        let mut mask = Mask::new(self.w, self.h);
        fill_into(&path, &mut mask, 0.0, 0.0);
        if let Some(outer) = self.clips.last() {
            for (a, b) in mask.a.iter_mut().zip(&outer.mask.a) {
                *a *= b;
            }
        }
        let (mut x0, mut y0, mut x1, mut y1) = (usize::MAX, usize::MAX, 0, 0);
        for y in 0..self.h {
            for x in 0..self.w {
                if mask.a[y * self.w + x] > 0.0 {
                    (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x + 1), y1.max(y + 1));
                }
            }
        }
        let bbox = if x0 == usize::MAX { (0, 0, 0, 0) } else { (x0, y0, x1, y1) };
        self.clips.push(Clip { mask, bbox });
    }

    /// A palette colour, premultiplied. 0xFFFF (the text's own colour) and missing entries are black.
    fn color(&self, palette_index: u16, alpha: f32) -> [f32; 4] {
        let c = self.palette.get(usize::from(palette_index)).copied().unwrap_or([0.0, 0.0, 0.0, 1.0]);
        let a = (c[3] * alpha).clamp(0.0, 1.0);
        [c[0] * a, c[1] * a, c[2] * a, a]
    }

    /// The premultiplied colour of a gradient at `t`.
    fn stop_color(&self, stops: &[ColorStop], extend: Extend, t: f32) -> [f32; 4] {
        let (Some(first), Some(last)) = (stops.first(), stops.last()) else { return [0.0; 4] };
        let t = match extend {
            Extend::Repeat => t - t.floor(),
            Extend::Reflect => {
                let r = t.rem_euclid(2.0);
                if r > 1.0 { 2.0 - r } else { r }
            }
            _ => t,
        };
        if t <= first.offset {
            return self.color(first.palette_index, first.alpha);
        }
        if t >= last.offset {
            return self.color(last.palette_index, last.alpha);
        }
        for pair in stops.windows(2) {
            let (a, b) = (pair[0], pair[1]);
            if t >= a.offset && t <= b.offset {
                let f = if b.offset > a.offset { (t - a.offset) / (b.offset - a.offset) } else { 0.0 };
                let (ca, cb) = (self.color(a.palette_index, a.alpha), self.color(b.palette_index, b.alpha));
                return [0, 1, 2, 3].map(|i| ca[i] + (cb[i] - ca[i]) * f);
            }
        }
        self.color(last.palette_index, last.alpha)
    }

    /// The brush's premultiplied colour at the point `(x, y)` of the brush's own space.
    fn brush_at(&self, brush: &Brush<'_>, x: f32, y: f32) -> [f32; 4] {
        match brush {
            Brush::Solid { palette_index, alpha } => self.color(*palette_index, *alpha),
            Brush::LinearGradient { p0, p1, color_stops, extend } => {
                let (dx, dy) = (p1.x - p0.x, p1.y - p0.y);
                let len2 = dx * dx + dy * dy;
                let t = if len2 > 0.0 { ((x - p0.x) * dx + (y - p0.y) * dy) / len2 } else { 0.0 };
                self.stop_color(color_stops, *extend, t)
            }
            Brush::RadialGradient { c0, r0, c1, r1, color_stops, extend } => {
                // two-point conical gradient: the largest t with |p - c(t)| = r(t) ≥ 0
                let (cdx, cdy, dr) = (c1.x - c0.x, c1.y - c0.y, r1 - r0);
                let (pdx, pdy) = (x - c0.x, y - c0.y);
                let a = cdx * cdx + cdy * cdy - dr * dr;
                let b = pdx * cdx + pdy * cdy + r0 * dr;
                let c = pdx * pdx + pdy * pdy - r0 * r0;
                let t = if a.abs() < 1e-6 {
                    if b.abs() < 1e-9 {
                        return [0.0; 4];
                    }
                    c / (2.0 * b)
                } else {
                    let disc = b * b - a * c;
                    if disc < 0.0 {
                        return [0.0; 4];
                    }
                    let s = disc.sqrt();
                    let (t1, t2) = ((b + s) / a, (b - s) / a);
                    let (hi, lo) = if t1 > t2 { (t1, t2) } else { (t2, t1) };
                    if r0 + hi * dr >= 0.0 {
                        hi
                    } else if r0 + lo * dr >= 0.0 {
                        lo
                    } else {
                        return [0.0; 4];
                    }
                };
                self.stop_color(color_stops, *extend, t)
            }
            Brush::SweepGradient { c0, start_angle, end_angle, color_stops, extend } => {
                // angles counter-clockwise in degrees, in the font's y-up space
                let angle = (y - c0.y).atan2(x - c0.x).to_degrees().rem_euclid(360.0);
                let span = end_angle - start_angle;
                let t = if span.abs() > 1e-6 { (angle - start_angle) / span } else { 0.0 };
                self.stop_color(color_stops, *extend, t)
            }
        }
    }

    /// Fill the current clip with `brush`, source-over onto the top layer.
    fn fill(&mut self, brush: &Brush<'_>) {
        let Some(inv) = invert(&self.current()) else { return };
        let (bx0, by0, bx1, by1) = self.clips.last().map_or((0, 0, self.w, self.h), |c| c.bbox);
        let mut out = Vec::with_capacity((bx1 - bx0) * (by1 - by0));
        for y in by0..by1 {
            for x in bx0..bx1 {
                let cover = self.clips.last().map_or(1.0, |c| c.mask.a[y * self.w + x]);
                if cover <= 0.0 {
                    out.push(None);
                    continue;
                }
                let (u, v) = apply(&inv, x as f32 + 0.5, y as f32 + 0.5);
                out.push(Some((cover, self.brush_at(brush, u, v))));
            }
        }
        let w = self.w;
        let Some(top) = self.layers.last_mut() else { return };
        let mut it = out.into_iter();
        for y in by0..by1 {
            for x in bx0..bx1 {
                if let Some(Some((cover, c))) = it.next() {
                    let d = &mut top.pixels[y * w + x];
                    *d = composite(CompositeMode::SrcOver, c.map(|v| v * cover), *d);
                }
            }
        }
    }
}

/// `s` (premultiplied) merged into `d` with a COLR composite mode.
fn composite(mode: CompositeMode, s: [f32; 4], d: [f32; 4]) -> [f32; 4] {
    let (sa, da) = (s[3], d[3]);
    let pd = |fs: f32, fd: f32| [0, 1, 2, 3].map(|i| s[i] * fs + d[i] * fd);
    match mode {
        CompositeMode::Clear => [0.0; 4],
        CompositeMode::Src => s,
        CompositeMode::Dest => d,
        CompositeMode::DestOver => pd(1.0 - da, 1.0),
        CompositeMode::SrcIn => pd(da, 0.0),
        CompositeMode::DestIn => pd(0.0, sa),
        CompositeMode::SrcOut => pd(1.0 - da, 0.0),
        CompositeMode::DestOut => pd(0.0, 1.0 - sa),
        CompositeMode::SrcAtop => pd(da, 1.0 - sa),
        CompositeMode::DestAtop => pd(1.0 - da, sa),
        CompositeMode::Xor => pd(1.0 - da, 1.0 - sa),
        CompositeMode::Plus => [0, 1, 2, 3].map(|i| (s[i] + d[i]).min(1.0)),
        CompositeMode::Multiply => {
            let o = [0, 1, 2].map(|i| s[i] * d[i] + s[i] * (1.0 - da) + d[i] * (1.0 - sa));
            [o[0], o[1], o[2], sa + da - sa * da]
        }
        CompositeMode::Screen => {
            let o = [0, 1, 2].map(|i| s[i] + d[i] - s[i] * d[i]);
            [o[0], o[1], o[2], sa + da - sa * da]
        }
        _ => pd(1.0, 1.0 - sa),
    }
}

impl ColorPainter for Painter<'_, '_> {
    fn push_transform(&mut self, transform: Transform) {
        let m = compose(&self.canvas.current(), &xform_of(&transform));
        self.canvas.xf.push(m);
    }

    fn pop_transform(&mut self) {
        if self.canvas.xf.len() > 1 {
            self.canvas.xf.pop();
        }
    }

    fn push_clip_glyph(&mut self, glyph_id: GlyphId) {
        let mut path = Path::new();
        if let Some(g) = self.font.outline_glyphs().get(glyph_id) {
            let mut pen = Pen { path: &mut path, m: self.canvas.current() };
            let _ = g.draw(DrawSettings::unhinted(Size::unscaled(), LocationRef::default()), &mut pen);
            path.close();
        }
        self.canvas.push_clip_path(path);
    }

    fn push_clip_box(&mut self, b: BoundingBox<f32>) {
        let m = self.canvas.current();
        let pts = [(b.x_min, b.y_min), (b.x_max, b.y_min), (b.x_max, b.y_max), (b.x_min, b.y_max)].map(|(x, y)| apply(&m, x, y));
        self.canvas.push_clip_path(Path::polygon(&pts));
    }

    fn pop_clip(&mut self) {
        self.canvas.clips.pop();
    }

    fn fill(&mut self, brush: Brush<'_>) {
        self.canvas.fill(&brush);
    }

    fn push_layer(&mut self, mode: CompositeMode) {
        let n = self.canvas.w * self.canvas.h;
        self.canvas.layers.push(Layer { pixels: vec![[0.0; 4]; n], mode });
    }

    fn pop_layer(&mut self) {
        let layers = &mut self.canvas.layers;
        if layers.len() < 2 {
            return;
        }
        let Some(layer) = layers.pop() else { return };
        let Some(below) = layers.last_mut() else { return };
        for (d, s) in below.pixels.iter_mut().zip(&layer.pixels) {
            *d = composite(layer.mode, *s, *d);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn porter_duff_and_blend_modes() {
        let red = [1.0, 0.0, 0.0, 1.0];
        let half_blue = [0.0, 0.0, 0.5, 0.5];
        assert_eq!(composite(CompositeMode::SrcOver, half_blue, red), [0.5, 0.0, 0.5, 1.0]);
        assert_eq!(composite(CompositeMode::DestIn, half_blue, red), [0.5, 0.0, 0.0, 0.5]);
        assert_eq!(composite(CompositeMode::Clear, half_blue, red), [0.0; 4]);
        assert_eq!(composite(CompositeMode::Multiply, [1.0, 1.0, 1.0, 1.0], red), red);
    }

    #[test]
    fn a_transform_inverts() {
        let m = [2.0, 0.5, -0.25, 3.0, 10.0, -4.0];
        let inv = invert(&m).unwrap();
        let (x, y) = apply(&m, 7.0, -2.0);
        let (u, v) = apply(&inv, x, y);
        assert!((u - 7.0).abs() < 1e-4 && (v + 2.0).abs() < 1e-4, "{u} {v}");
        assert!(invert(&[0.0; 6]).is_none());
    }

    /// Windows's emoji font draws a colourful face.
    #[cfg(target_os = "windows")]
    #[test]
    fn segoe_ui_emoji_draws_a_yellow_face() {
        assert!(is_color_family("Segoe UI Emoji"));
        let Some((w, h, px)) = render("😀", "Segoe UI Emoji", 128.0) else { panic!("no colour glyph") };
        let opaque: Vec<&[u8]> = px.chunks(4).filter(|p| p[3] > 200).collect();
        assert!(opaque.len() > (w * h / 8) as usize, "{} of {w}×{h} opaque", opaque.len());
        let yellow = opaque.iter().filter(|p| p[0] > 180 && p[1] > 120 && p[2] < 120).count();
        assert!(yellow > opaque.len() / 4, "a yellow face: {yellow} of {}", opaque.len());
    }
}
