//! Faces in a picture, with Apple Vision on macOS (`VNDetectFaceRectanglesRequest`), for framing a
//! clip on its speaker (Auto Reframe to faces).
//!
//! On a wide shot a face can be 60–70 px of a 4K frame, too small for Vision on the whole picture,
//! so the picture is cut into nine overlapping tiles (half the frame each, a quarter apart), each is
//! searched, the boxes are brought back to fractions of the whole picture and duplicates from
//! neighbouring tiles are merged (intersection over union above 0.3 keeps the more confident box),
//! as Jilo Studio does. Other systems report that face detection is not available.
//!
//! The subject of a picture cut out from its background (`VNGenerateForegroundInstanceMaskRequest`,
//! macOS 14+): people, animals and things in front, as a picture with transparency — for stickers
//! made from the user's own photos and frames.
//!
//! What a picture shows, as Vision's scene labels (`VNClassifyImageRequest`: "beach", "food",
//! "people"…), for finding B-roll that fits what is being said.

/// A face: its box in fractions of the picture, `y` from the top, and Vision's confidence (0–1).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Face {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub confidence: f32,
}

impl Face {
    fn area(&self) -> f32 {
        self.w.max(0.0) * self.h.max(0.0)
    }

    /// Intersection over union of two boxes.
    pub fn overlap(&self, o: &Face) -> f32 {
        let ix = ((self.x + self.w).min(o.x + o.w) - self.x.max(o.x)).max(0.0);
        let iy = ((self.y + self.h).min(o.y + o.h) - self.y.max(o.y)).max(0.0);
        let inter = ix * iy;
        let union = self.area() + o.area() - inter;
        if union > 0.0 { inter / union } else { 0.0 }
    }
}

/// Add `face` to `faces`, merging it with a box it overlaps by more than 0.3 (the more confident
/// one stays).
pub fn merge_face(faces: &mut Vec<Face>, face: Face) {
    match faces.iter_mut().find(|f| f.overlap(&face) > 0.3) {
        Some(f) if face.confidence > f.confidence => *f = face,
        Some(_) => {}
        None => faces.push(face),
    }
}

/// The tiles a picture is searched in: (x, y, w, h) in pixels, a 3×3 grid of half-size tiles a
/// quarter apart.
pub fn tiles(w: usize, h: usize) -> Vec<(usize, usize, usize, usize)> {
    let (tw, th) = ((w / 2).max(1), (h / 2).max(1));
    let mut out = Vec::with_capacity(9);
    for row in 0..3 {
        for col in 0..3 {
            let (x, y) = (col * w / 4, row * h / 4);
            out.push((x, y, tw.min(w - x.min(w)), th.min(h - y.min(h))));
        }
    }
    out
}

/// Whether this system can find faces.
pub fn available() -> bool {
    cfg!(target_os = "macos")
}

/// Faces in an 8-bit RGBA picture of `w`×`h` (rows of `w * 4` bytes).
pub fn detect_faces(rgba: &[u8], w: usize, h: usize) -> Result<Vec<Face>, String> {
    let need = w.checked_mul(h).and_then(|n| n.checked_mul(4)).ok_or("the picture is too large")?;
    if w < 8 || h < 8 || rgba.len() < need {
        return Err(format!("a {w}×{h} picture needs {need} bytes of RGBA"));
    }
    imp::detect(&rgba[..need], w, h)
}

/// Labels of what an 8-bit RGBA picture of `w`×`h` shows, most confident first: at most `max`,
/// each at least `min_confidence` (0–1).
pub fn classify_image(rgba: &[u8], w: usize, h: usize, max: usize, min_confidence: f32) -> Result<Vec<(String, f32)>, String> {
    let need = w.checked_mul(h).and_then(|n| n.checked_mul(4)).ok_or("the picture is too large")?;
    if w < 8 || h < 8 || rgba.len() < need {
        return Err(format!("a {w}×{h} picture needs {need} bytes of RGBA"));
    }
    let mut v = imp::classify(&rgba[..need], w, h)?;
    v.retain(|(l, c)| *c >= min_confidence && !l.is_empty());
    v.sort_by(|a, b| b.1.total_cmp(&a.1));
    v.truncate(max);
    Ok(v)
}

/// Whether this system can cut a picture's subject out (Vision's foreground mask, macOS 14).
pub fn cut_out_available() -> bool {
    #[cfg(target_os = "macos")]
    return objc2::runtime::AnyClass::get(c"VNGenerateForegroundInstanceMaskRequest").is_some();
    #[cfg(not(target_os = "macos"))]
    false
}

/// The foreground of an 8-bit RGBA picture of `w`×`h` with the background made transparent, cropped
/// to it: (width, height, straight RGBA). An error when nothing stands out.
pub fn cut_out(rgba: &[u8], w: usize, h: usize) -> Result<(u32, u32, Vec<u8>), String> {
    let need = w.checked_mul(h).and_then(|n| n.checked_mul(4)).ok_or("the picture is too large")?;
    if w < 8 || h < 8 || rgba.len() < need {
        return Err(format!("a {w}×{h} picture needs {need} bytes of RGBA"));
    }
    if !cut_out_available() {
        return Err("cutting out needs macOS 14 or later".into());
    }
    let mask = imp::foreground_mask(&rgba[..need], w, h)?;
    apply_mask(&rgba[..need], w, h, &mask).ok_or_else(|| "nothing stands out from the background".to_string())
}

/// `rgba` with `mask` (0–1 per pixel) as its alpha, cropped to where the mask is.
pub fn apply_mask(rgba: &[u8], w: usize, h: usize, mask: &[f32]) -> Option<(u32, u32, Vec<u8>)> {
    let (mut x0, mut y0, mut x1, mut y1) = (usize::MAX, usize::MAX, 0usize, 0usize);
    for y in 0..h {
        for x in 0..w {
            if mask.get(y * w + x).copied().unwrap_or(0.0) > 0.05 {
                (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x), y1.max(y));
            }
        }
    }
    if x0 == usize::MAX {
        return None;
    }
    let (cw, ch) = (x1 - x0 + 1, y1 - y0 + 1);
    let mut out = Vec::with_capacity(cw * ch * 4);
    for y in y0..=y1 {
        for x in x0..=x1 {
            let i = (y * w + x) * 4;
            let a = (mask[y * w + x].clamp(0.0, 1.0) * f32::from(rgba[i + 3])).round() as u8;
            out.extend_from_slice(&[rgba[i], rgba[i + 1], rgba[i + 2], a]);
        }
    }
    Some((cw as u32, ch as u32, out))
}

#[cfg(not(target_os = "macos"))]
mod imp {
    pub fn foreground_mask(_: &[u8], _: usize, _: usize) -> Result<Vec<f32>, String> {
        Err("cutting out needs macOS (Apple Vision)".into())
    }
    pub fn detect(_: &[u8], _: usize, _: usize) -> Result<Vec<super::Face>, String> {
        Err("face detection needs macOS (Apple Vision)".into())
    }
    pub fn classify(_: &[u8], _: usize, _: usize) -> Result<Vec<(String, f32)>, String> {
        Err("scene labels need macOS (Apple Vision)".into())
    }
}

#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
mod imp {
    use super::{Face, merge_face, tiles};
    use objc2::AnyThread;
    use objc2::rc::autoreleasepool;
    use objc2_core_foundation::{CFData, CGPoint, CGRect, CGSize};
    use objc2_core_graphics::{CGBitmapInfo, CGColorRenderingIntent, CGColorSpace, CGDataProvider, CGImage, CGImageAlphaInfo};
    use objc2_core_video::{
        CVPixelBufferGetBaseAddress, CVPixelBufferGetBytesPerRow, CVPixelBufferGetHeight, CVPixelBufferGetPixelFormatType, CVPixelBufferGetWidth,
        CVPixelBufferLockBaseAddress, CVPixelBufferLockFlags, CVPixelBufferUnlockBaseAddress,
    };
    use objc2_foundation::{NSArray, NSDictionary};
    use objc2_vision::{VNClassifyImageRequest, VNDetectFaceRectanglesRequest, VNGenerateForegroundInstanceMaskRequest, VNImageRequestHandler, VNRequest};

    /// kCVPixelFormatType_OneComponent32Float ('L00f') and OneComponent8 ('L008').
    const F32: u32 = u32::from_be_bytes(*b"L00f");
    const U8: u32 = u32::from_be_bytes(*b"L008");

    /// The foreground mask (0–1 per pixel, `w`×`h`) of every instance Vision finds.
    pub fn foreground_mask(rgba: &[u8], w: usize, h: usize) -> Result<Vec<f32>, String> {
        autoreleasepool(|_| {
            let image = cg_image(rgba, w, h)?;
            // SAFETY: a plain `new` of a Vision request class (no arguments).
            let request = unsafe { VNGenerateForegroundInstanceMaskRequest::new() };
            let options = NSDictionary::new();
            // SAFETY: `image` is a valid CGImage and `options` an empty dictionary of the declared
            // key and value types.
            let handler = unsafe { VNImageRequestHandler::initWithCGImage_options(VNImageRequestHandler::alloc(), &image, &options) };
            let as_request: objc2::rc::Retained<VNRequest> = objc2::rc::Retained::into_super(objc2::rc::Retained::into_super(request.clone()));
            let requests = NSArray::from_retained_slice(&[as_request]);
            handler.performRequests_error(&requests).map_err(|e| format!("Vision could not cut out the picture: {e}"))?;
            // SAFETY: the request has been performed; `results` is its NSArray of observations.
            let obs = unsafe { request.results() }.and_then(|r| r.firstObject()).ok_or("nothing stands out from the background")?;
            // SAFETY: plain reads of the observation, and a mask of its instances at the image's
            // size for the handler that analysed it.
            let pb = unsafe {
                let all = obs.allInstances();
                obs.generateScaledMaskForImageForInstances_fromRequestHandler_error(&all, &handler)
                    .map_err(|e| format!("Vision could not make the mask: {e}"))?
            };
            // SAFETY: the buffer is locked read-only while its rows are read, and unlocked after.
            unsafe {
                if CVPixelBufferLockBaseAddress(&pb, CVPixelBufferLockFlags(1)) != 0 {
                    return Err("the mask could not be read".to_string());
                }
                let (mw, mh, row, fmt) =
                    (CVPixelBufferGetWidth(&pb), CVPixelBufferGetHeight(&pb), CVPixelBufferGetBytesPerRow(&pb), CVPixelBufferGetPixelFormatType(&pb));
                let base = CVPixelBufferGetBaseAddress(&pb).cast::<u8>();
                let mut mask = vec![0f32; w * h];
                if !base.is_null() && mw > 0 && mh > 0 {
                    for y in 0..h {
                        let my = (y * mh / h).min(mh - 1);
                        let line = base.add(my * row);
                        for x in 0..w {
                            let mx = (x * mw / w).min(mw - 1);
                            mask[y * w + x] = match fmt {
                                F32 => line.add(mx * 4).cast::<f32>().read_unaligned(),
                                U8 => f32::from(*line.add(mx)) / 255.0,
                                _ => 0.0,
                            };
                        }
                    }
                }
                CVPixelBufferUnlockBaseAddress(&pb, CVPixelBufferLockFlags(1));
                if !matches!(fmt, F32 | U8) {
                    return Err(format!("unexpected mask format {fmt:#x}"));
                }
                Ok(mask)
            }
        })
    }

    /// A CGImage over a copy of 8-bit RGBA rows (the alpha is ignored).
    fn cg_image(rgba: &[u8], w: usize, h: usize) -> Result<objc2_core_foundation::CFRetained<CGImage>, String> {
        let len = isize::try_from(rgba.len()).map_err(|_| "the picture is too large".to_string())?;
        // SAFETY: `rgba` is a live slice of `len` bytes; CFDataCreate copies them.
        let data = unsafe { CFData::new(None, rgba.as_ptr(), len) }.ok_or("CoreFoundation could not hold the picture")?;
        let provider = CGDataProvider::with_cf_data(Some(&data)).ok_or("CoreGraphics could not read the picture")?;
        let space = CGColorSpace::new_device_rgb().ok_or("no RGB colour space")?;
        // SAFETY: the provider holds w*h*4 bytes laid out as 8-bit RGBX rows of w*4 bytes, as
        // declared; `decode` may be null.
        unsafe {
            CGImage::new(
                w,
                h,
                8,
                32,
                w * 4,
                Some(&space),
                CGBitmapInfo(CGImageAlphaInfo::NoneSkipLast.0),
                Some(&provider),
                std::ptr::null(),
                false,
                CGColorRenderingIntent::RenderingIntentDefault,
            )
        }
        .ok_or_else(|| "CoreGraphics could not make an image".to_string())
    }

    pub fn classify(rgba: &[u8], w: usize, h: usize) -> Result<Vec<(String, f32)>, String> {
        autoreleasepool(|_| {
            let image = cg_image(rgba, w, h)?;
            // SAFETY: a plain `new` of a Vision request class (no arguments).
            let request = unsafe { VNClassifyImageRequest::new() };
            let options = NSDictionary::new();
            // SAFETY: `image` is a valid CGImage and `options` an empty dictionary of the declared
            // key and value types.
            let handler = unsafe { VNImageRequestHandler::initWithCGImage_options(VNImageRequestHandler::alloc(), &image, &options) };
            let as_request: objc2::rc::Retained<VNRequest> = objc2::rc::Retained::into_super(objc2::rc::Retained::into_super(request.clone()));
            let requests = NSArray::from_retained_slice(&[as_request]);
            handler.performRequests_error(&requests).map_err(|e| format!("Vision could not label the picture: {e}"))?;
            // SAFETY: the request has been performed; `results` is its NSArray of observations.
            let Some(results) = (unsafe { request.results() }) else { return Ok(Vec::new()) };
            Ok(results
                .iter()
                // SAFETY: plain property reads of a classification observation Vision returned.
                .map(|obs| unsafe { (obs.identifier().to_string(), obs.confidence()) })
                .collect())
        })
    }

    pub fn detect(rgba: &[u8], w: usize, h: usize) -> Result<Vec<Face>, String> {
        autoreleasepool(|_| {
            let image = cg_image(rgba, w, h)?;
            let mut faces: Vec<Face> = Vec::new();
            for (tx, ty, tw, th) in tiles(w, h) {
                let rect = CGRect { origin: CGPoint { x: tx as f64, y: ty as f64 }, size: CGSize { width: tw as f64, height: th as f64 } };
                let Some(tile) = CGImage::with_image_in_rect(Some(&image), rect) else { continue };
                // SAFETY: a plain `new` of a Vision request class (no arguments).
                let request = unsafe { VNDetectFaceRectanglesRequest::new() };
                let options = NSDictionary::new();
                // SAFETY: `tile` is a valid CGImage and `options` an empty dictionary of the
                // declared key and value types.
                let handler = unsafe { VNImageRequestHandler::initWithCGImage_options(VNImageRequestHandler::alloc(), &tile, &options) };
                let as_request: objc2::rc::Retained<VNRequest> = objc2::rc::Retained::into_super(objc2::rc::Retained::into_super(request.clone()));
                let requests = NSArray::from_retained_slice(&[as_request]);
                if handler.performRequests_error(&requests).is_err() {
                    continue;
                }
                // SAFETY: the request has been performed; `results` is its NSArray of observations.
                let Some(results) = (unsafe { request.results() }) else { continue };
                for obs in results.iter() {
                    // SAFETY: plain property reads of a face observation Vision returned.
                    let (b, c) = unsafe { (obs.boundingBox(), obs.confidence()) };
                    // Vision boxes are normalised to the tile with the origin at the bottom left
                    let face = Face {
                        x: ((tx as f64 + b.origin.x * tw as f64) / w as f64) as f32,
                        y: ((ty as f64 + (1.0 - b.origin.y - b.size.height) * th as f64) / h as f64) as f32,
                        w: (b.size.width * tw as f64 / w as f64) as f32,
                        h: (b.size.height * th as f64 / h as f64) as f32,
                        confidence: c,
                    };
                    if face.w > 0.0 && face.h > 0.0 && face.x.is_finite() && face.y.is_finite() {
                        merge_face(&mut faces, face);
                    }
                }
            }
            Ok(faces)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn face(x: f32, y: f32, s: f32, c: f32) -> Face {
        Face { x, y, w: s, h: s, confidence: c }
    }

    #[test]
    fn duplicates_from_neighbouring_tiles_merge() {
        let mut faces = Vec::new();
        merge_face(&mut faces, face(0.40, 0.30, 0.10, 0.7));
        merge_face(&mut faces, face(0.41, 0.31, 0.10, 0.9)); // the same face from the next tile
        merge_face(&mut faces, face(0.70, 0.30, 0.10, 0.8)); // someone else
        assert_eq!(faces.len(), 2);
        assert_eq!(faces[0].confidence, 0.9, "the more confident box stays");
        assert!(face(0.0, 0.0, 0.1, 1.0).overlap(&face(0.5, 0.5, 0.1, 1.0)) == 0.0);
        assert!((face(0.0, 0.0, 0.2, 1.0).overlap(&face(0.0, 0.0, 0.2, 1.0)) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn tiles_cover_the_picture_with_halves_a_quarter_apart() {
        let t = tiles(3840, 2160);
        assert_eq!(t.len(), 9);
        assert_eq!(t[0], (0, 0, 1920, 1080));
        assert_eq!(t[4], (960, 540, 1920, 1080));
        assert_eq!(t[8], (1920, 1080, 1920, 1080));
        // hostile sizes never panic
        let _ = tiles(1, 1);
        assert!(detect_faces(&[0; 10], 4, 4).is_err());
        assert!(detect_faces(&[], usize::MAX, 2).is_err());
    }

    /// Vision labels a picture: a plain sky-blue picture gets some labels, all within 0–1.
    #[cfg(target_os = "macos")]
    #[test]
    fn vision_labels_a_picture() {
        let mut rgba = Vec::with_capacity(320 * 240 * 4);
        for y in 0..240 {
            for _ in 0..320 {
                rgba.extend_from_slice(if y < 140 { &[110, 170, 235, 255] } else { &[30, 110, 60, 255] });
            }
        }
        let labels = classify_image(&rgba, 320, 240, 5, 0.0).unwrap();
        assert!(labels.len() <= 5);
        assert!(labels.iter().all(|(l, c)| !l.is_empty() && (0.0..=1.0).contains(c)), "{labels:?}");
        assert!(labels.windows(2).all(|p| p[0].1 >= p[1].1), "most confident first");
        assert!(classify_image(&[0; 10], 4, 4, 5, 0.0).is_err());
    }

    #[test]
    fn a_mask_makes_the_background_clear_and_crops() {
        let rgba = vec![200u8; 4 * 4 * 4];
        let mut mask = vec![0f32; 16];
        mask[5] = 1.0;
        mask[6] = 0.5;
        let (w, h, out) = apply_mask(&rgba, 4, 4, &mask).unwrap();
        assert_eq!((w, h), (2, 1));
        assert_eq!((out[3], out[7]), (200, 100));
        assert!(apply_mask(&rgba, 4, 4, &[0.0; 16]).is_none());
        assert!(cut_out(&[0; 10], 4, 4).is_err());
    }

    /// Vision cuts a bright disc out of a dark background.
    #[cfg(target_os = "macos")]
    #[test]
    fn vision_cuts_out_a_subject() {
        let (w, h) = (256usize, 256usize);
        let mut rgba = Vec::with_capacity(w * h * 4);
        for y in 0..h {
            for x in 0..w {
                let d = ((x as f32 - 128.0).powi(2) + (y as f32 - 128.0).powi(2)).sqrt();
                rgba.extend_from_slice(if d < 60.0 { &[230, 60, 40, 255] } else { &[20, 30, 40, 255] });
            }
        }
        match cut_out(&rgba, w, h) {
            // the subject is the disc: cropped near its size, its middle opaque, corners clear
            Ok((cw, ch, out)) => {
                assert!(cw < 200 && ch < 200, "{cw}×{ch}");
                let mid = ((ch / 2 * cw + cw / 2) * 4 + 3) as usize;
                assert!(out[mid] > 200, "the disc stays");
            }
            // a plain synthetic picture may hold no subject for Vision
            Err(e) => assert!(e.contains("nothing stands out"), "{e}"),
        }
    }

    /// A blank picture has no faces (Vision runs, finds nothing).
    #[cfg(target_os = "macos")]
    #[test]
    fn vision_finds_no_face_in_a_blank_picture() {
        let rgba = vec![128u8; 320 * 180 * 4];
        assert_eq!(detect_faces(&rgba, 320, 180).unwrap(), Vec::new());
    }
}
