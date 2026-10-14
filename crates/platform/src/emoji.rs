//! Emoji as pictures, drawn on this computer with the system's colour emoji font (Apple Color
//! Emoji) for stickers: nothing is downloaded or shipped, every emoji the system knows can be a
//! sticker. The picture is trimmed to the emoji with a small margin. Other systems report that
//! emoji pictures are not available.

/// Whether this system can draw emoji pictures.
pub fn available() -> bool {
    cfg!(target_os = "macos")
}

/// The emoji `text` drawn `size` pixels tall (16–1024): (width, height, RGBA rows, straight alpha).
pub fn render(text: &str, size: u32) -> Result<(u32, u32, Vec<u8>), String> {
    if text.trim().is_empty() || text.chars().count() > 16 {
        return Err("an emoji is one to a few characters".into());
    }
    let size = size.clamp(16, 1024);
    let (w, h, mut px) = imp::render(text, size)?;
    // premultiplied → straight alpha
    for p in px.as_chunks_mut::<4>().0 {
        let a = u32::from(p[3]);
        if a > 0 && a < 255 {
            for c in &mut p[..3] {
                *c = ((u32::from(*c) * 255 + a / 2) / a).min(255) as u8;
            }
        }
    }
    trim(w, h, &px).ok_or_else(|| format!("the system has no picture for `{text}`"))
}

/// The smallest box around the opaque pixels, with a margin of 4 % of the canvas.
fn trim(w: u32, h: u32, px: &[u8]) -> Option<(u32, u32, Vec<u8>)> {
    let (w, h) = (w as usize, h as usize);
    let mut b = (usize::MAX, usize::MAX, 0usize, 0usize);
    for y in 0..h {
        for x in 0..w {
            if px[(y * w + x) * 4 + 3] > 8 {
                b = (b.0.min(x), b.1.min(y), b.2.max(x), b.3.max(y));
            }
        }
    }
    if b.0 == usize::MAX {
        return None;
    }
    let m = (w.max(h) / 25).max(1);
    let (x0, y0, x1, y1) = (b.0.saturating_sub(m), b.1.saturating_sub(m), (b.2 + m).min(w - 1), (b.3 + m).min(h - 1));
    let (tw, th) = (x1 - x0 + 1, y1 - y0 + 1);
    let mut out = Vec::with_capacity(tw * th * 4);
    for y in y0..=y1 {
        out.extend_from_slice(&px[(y * w + x0) * 4..(y * w + x1 + 1) * 4]);
    }
    Some((tw as u32, th as u32, out))
}

#[cfg(not(target_os = "macos"))]
mod imp {
    pub fn render(_: &str, _: u32) -> Result<(u32, u32, Vec<u8>), String> {
        Err("emoji pictures need macOS".into())
    }
}

#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
mod imp {
    use objc2::rc::autoreleasepool;
    use objc2::runtime::AnyObject;
    use objc2_app_kit::{NSFont, NSFontAttributeName, NSGraphicsContext, NSStringDrawing};
    use objc2_core_foundation::CGPoint;
    use objc2_core_graphics::{CGBitmapContextCreate, CGColorSpace, CGImageAlphaInfo};
    use objc2_foundation::{NSDictionary, NSString};

    pub fn render(text: &str, size: u32) -> Result<(u32, u32, Vec<u8>), String> {
        // a square canvas with room around the glyph (emoji draw a little above the baseline)
        let side = (f64::from(size) * 1.5).ceil() as usize;
        let mut px = vec![0u8; side * side * 4];
        autoreleasepool(|_| {
            let space = CGColorSpace::new_device_rgb().ok_or("no RGB colour space")?;
            // SAFETY: `px` holds side*side*4 bytes, rows of side*4 bytes, and outlives the
            // context, which is dropped inside this closure.
            let ctx = unsafe { CGBitmapContextCreate(px.as_mut_ptr().cast(), side, side, 8, side * 4, Some(&space), CGImageAlphaInfo::PremultipliedLast.0) }
                .ok_or("CoreGraphics could not make a drawing surface")?;
            let ns = NSGraphicsContext::graphicsContextWithCGContext_flipped(&ctx, false);
            let font = NSFont::systemFontOfSize(f64::from(size));
            let s = NSString::from_str(text);
            // SAFETY: the key is AppKit's font attribute and the value an NSFont, as the
            // dictionary's declared key and value types expect.
            let key = unsafe { NSFontAttributeName };
            let font_obj: &AnyObject = &font;
            let attrs = NSDictionary::from_slices(&[key], &[font_obj]);
            NSGraphicsContext::saveGraphicsState_class();
            NSGraphicsContext::setCurrentContext(Some(&ns));
            let origin = CGPoint { x: f64::from(size) * 0.2, y: f64::from(size) * 0.3 };
            // SAFETY: the current graphics context draws into `px`; the attributes are the
            // dictionary built above.
            unsafe { s.drawAtPoint_withAttributes(origin, Some(&attrs)) };
            NSGraphicsContext::restoreGraphicsState_class();
            drop(ctx);
            Ok::<(), String>(())
        })?;
        Ok((side as u32, side as u32, px))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn margins_are_trimmed() {
        let mut px = vec![0u8; 10 * 10 * 4];
        px[(5 * 10 + 5) * 4 + 3] = 255;
        let (w, h, out) = trim(10, 10, &px).unwrap();
        assert_eq!((w, h), (3, 3));
        assert_eq!(out.len(), 3 * 3 * 4);
        assert!(trim(10, 10, &vec![0u8; 400]).is_none());
        assert!(render("", 64).is_err());
    }

    /// The system draws a colourful emoji: many opaque pixels, more than one colour.
    #[cfg(target_os = "macos")]
    #[test]
    fn an_emoji_is_drawn_in_colour() {
        let (w, h, px) = render("😀", 128).unwrap();
        assert!(w > 60 && h > 60 && w < 200 && h < 200, "{w}×{h}");
        let opaque: Vec<&[u8]> = px.chunks(4).filter(|p| p[3] > 200).collect();
        assert!(opaque.len() > (w * h / 4) as usize, "mostly the face");
        let yellow = opaque.iter().filter(|p| p[0] > 200 && p[1] > 150 && p[2] < 120).count();
        assert!(yellow > opaque.len() / 4, "a yellow face: {yellow} of {}", opaque.len());
    }
}
