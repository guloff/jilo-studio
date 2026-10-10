//! VideoToolbox (macOS) hardware ProRes encoding: 422 Proxy / LT / 422 / HQ and 4444 / 4444 XQ,
//! with alpha.
//!
//! An FFI module of this crate (docs/adr/0001-platform-ffi.md): every `unsafe` block has a
//! `// SAFETY:` comment, no panic may unwind into VideoToolbox (the output callback runs under
//! `catch_unwind`), and every failure is a `Result` the caller answers by using the built-in
//! encoder instead.
//!
//! A compression session is created with a hardware encoder *required* (the ProRes engine of
//! Apple silicon Pro / Max chips), so a machine without one is declined up front. ProRes is
//! intra-only: every compressed frame is a sync sample, in presentation order, with no parameter
//! sets; its size is set by the profile (there is no bitrate to control).
//!
//! Pictures go in as the planes the built-in encoder makes, so colours match it exactly: 422 as
//! 10-bit 4:2:2 (`x422`, from `rgba_to_yuv422_10`), 4444 as 10-bit 4:4:4 (`x444`) or, with
//! alpha, packed 16-bit A Y'CbCr (`y416`) from the 12-bit planes of `rgba_to_yuv444_12`.

use std::ffi::c_void;
use std::panic::AssertUnwindSafe;
use std::ptr::NonNull;
use std::sync::{Mutex, PoisonError};

use objc2_core_foundation::{CFBoolean, CFDictionary, CFNumber, CFRetained, CFString, CFType};
use objc2_core_media::{CMSampleBuffer, CMTime, CMTimeFlags};
use objc2_core_video::{
    CVImageBuffer, CVPixelBufferCreate, CVPixelBufferGetBaseAddress, CVPixelBufferGetBaseAddressOfPlane, CVPixelBufferGetBytesPerRow,
    CVPixelBufferGetBytesPerRowOfPlane, CVPixelBufferGetHeight, CVPixelBufferGetHeightOfPlane, CVPixelBufferGetWidth, CVPixelBufferGetWidthOfPlane,
    CVPixelBufferLockBaseAddress, CVPixelBufferLockFlags, CVPixelBufferPool, CVPixelBufferUnlockBaseAddress, kCVImageBufferColorPrimaries_ITU_R_709_2,
    kCVImageBufferColorPrimaries_ITU_R_2020, kCVImageBufferTransferFunction_ITU_R_709_2, kCVImageBufferTransferFunction_ITU_R_2100_HLG,
    kCVImageBufferTransferFunction_SMPTE_ST_2084_PQ, kCVImageBufferYCbCrMatrix_ITU_R_709_2, kCVImageBufferYCbCrMatrix_ITU_R_2020, kCVPixelBufferHeightKey,
    kCVPixelBufferIOSurfacePropertiesKey, kCVPixelBufferPixelFormatTypeKey, kCVPixelBufferWidthKey,
};
use objc2_video_toolbox::{
    VTCompressionSession, VTEncodeInfoFlags, VTSessionCopyProperty, VTSessionSetProperty, kVTCompressionPropertyKey_ColorPrimaries,
    kVTCompressionPropertyKey_ExpectedFrameRate, kVTCompressionPropertyKey_RealTime, kVTCompressionPropertyKey_TransferFunction,
    kVTCompressionPropertyKey_UsingHardwareAcceleratedVideoEncoder, kVTCompressionPropertyKey_YCbCrMatrix,
    kVTVideoEncoderSpecification_RequireHardwareAcceleratedVideoEncoder,
};

/// 10-bit 4:2:2 / 4:4:4 biplanar video range (MSB-aligned 16-bit samples, CbCr interleaved).
const X422: u32 = u32::from_be_bytes(*b"x422");
const X444: u32 = u32::from_be_bytes(*b"x444");
/// Packed 4:4:4:4 A Y'CbCr, 16 bits each, little-endian.
const Y416: u32 = u32::from_be_bytes(*b"y416");
/// Largest compressed frame accepted from the callback (a sanity cap on a size we do not control).
const MAX_FRAME_BYTES: usize = 512 << 20;
/// Largest picture the hardware encoder is asked for.
const MAX_SIDE: u32 = 8192;

/// The colour signal written to the frames: BT.709, or BT.2020 with PQ or HLG.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProResSignal {
    Bt709,
    Pq,
    Hlg,
}

/// What to encode. Times are in a timescale of `timescale` units per second, one frame lasting
/// `frame_duration` units.
#[derive(Clone, Debug)]
pub struct ProResConfig {
    pub width: u32,
    pub height: u32,
    pub timescale: u32,
    pub frame_duration: u32,
    /// `apco` / `apcs` / `apcn` / `apch` / `ap4h` / `ap4x`.
    pub fourcc: [u8; 4],
    /// 4444 only: the pictures carry an alpha plane to encode.
    pub alpha: bool,
    pub signal: ProResSignal,
}

impl ProResConfig {
    /// A 4444 / 4444 XQ profile (4:4:4).
    pub fn is_4444(&self) -> bool {
        matches!(&self.fourcc, b"ap4h" | b"ap4x")
    }

    fn pixel_format(&self) -> u32 {
        match (self.is_4444(), self.alpha) {
            (false, _) => X422,
            (true, false) => X444,
            (true, true) => Y416,
        }
    }
}

/// One compressed frame (a complete ProRes frame, the MOV sample), in presentation order.
#[derive(Clone, Debug)]
pub struct ProResPacket {
    pub data: Vec<u8>,
    pub pts: i64,
}

enum Output {
    Packet(ProResPacket),
    Failed(String),
}

/// State shared with the output callback (its refcon), kept alive until the session is invalidated.
struct Shared {
    timescale: i64,
    out: Mutex<Vec<Output>>,
}

/// A compression session with its callback state. Dropping it completes the pending frames and
/// invalidates the session before the callback state is freed.
struct Session {
    session: CFRetained<VTCompressionSession>,
    /// Pointed to by the session's callback: must outlive `session`.
    shared: Box<Shared>,
}

// SAFETY: a VTCompressionSession and the CoreMedia / CoreVideo objects it uses may be used from
// any thread (Apple: "thread safe"); the encoder owning a `Session` is used by one thread at a time
// (`&mut self`), and the callback state is behind a `Mutex`.
unsafe impl Send for Session {}

impl Drop for Session {
    fn drop(&mut self) {
        // SAFETY: the session is valid until released. Completing the frames lets any callback in
        // flight finish; invalidating guarantees none runs afterwards, so `shared` can be freed.
        unsafe {
            let _ = self.session.complete_frames(invalid_time());
            self.session.invalidate();
        }
    }
}

fn invalid_time() -> CMTime {
    CMTime { value: 0, timescale: 0, flags: CMTimeFlags(0), epoch: 0 }
}

fn cm_time(value: i64, timescale: u32) -> CMTime {
    CMTime { value, timescale: timescale.min(i32::MAX as u32) as i32, flags: CMTimeFlags::Valid, epoch: 0 }
}

/// `t` in units of `1 / timescale` s, rounded to nearest; None for an invalid time.
fn ticks(t: CMTime, timescale: i64) -> Option<i64> {
    let (value, scale, flags) = (t.value, t.timescale, t.flags);
    if !flags.contains(CMTimeFlags::Valid) || scale <= 0 {
        return None;
    }
    let (num, den) = (i128::from(value) * i128::from(timescale), i128::from(scale));
    i64::try_from((2 * num + den).div_euclid(2 * den)).ok()
}

fn read_packet(sample: &CMSampleBuffer, shared: &Shared) -> Result<ProResPacket, String> {
    // SAFETY: `sample` is a valid sample buffer for the duration of the callback.
    let block = unsafe { sample.data_buffer() }.ok_or("compressed frame without data")?;
    // SAFETY: `block` is a valid block buffer (retained above).
    let len = unsafe { block.data_length() };
    if !(8..=MAX_FRAME_BYTES).contains(&len) {
        return Err(format!("compressed frame of {len} bytes"));
    }
    let mut data = vec![0u8; len];
    let dest = NonNull::new(data.as_mut_ptr().cast::<c_void>()).ok_or("empty frame buffer")?;
    // SAFETY: `dest` points to `len` writable bytes, the buffer's own length.
    let status = unsafe { block.copy_data_bytes(0, len, dest) };
    if status != 0 {
        return Err(format!("cannot copy a compressed frame ({status})"));
    }
    if data.get(4..8) != Some(b"icpf".as_slice()) {
        return Err("VideoToolbox returned something other than a ProRes frame".into());
    }
    // SAFETY: valid sample buffer, see above.
    let pts = ticks(unsafe { sample.presentation_time_stamp() }, shared.timescale).ok_or("compressed frame without a presentation time")?;
    Ok(ProResPacket { data, pts })
}

/// The output callback. Never unwinds: panics are caught and reported as a failed frame.
unsafe extern "C-unwind" fn output_callback(refcon: *mut c_void, _frame: *mut c_void, status: i32, flags: VTEncodeInfoFlags, sample: *mut CMSampleBuffer) {
    if refcon.is_null() {
        return;
    }
    // SAFETY: `refcon` is the `Shared` the session was created with; it lives in a `Box` owned by
    // the `Session`, which invalidates the session (no more callbacks) before freeing it.
    let shared = unsafe { &*(refcon as *const Shared) };
    let result = std::panic::catch_unwind(AssertUnwindSafe(|| {
        if status != 0 {
            return Output::Failed(format!("VideoToolbox ProRes encode error {status}"));
        }
        if flags.contains(VTEncodeInfoFlags::FrameDropped) {
            return Output::Failed("VideoToolbox dropped a frame".into());
        }
        let Some(sample) = NonNull::new(sample) else {
            return Output::Failed("VideoToolbox returned no data for a frame".into());
        };
        // SAFETY: VideoToolbox passes a valid sample buffer for the duration of the callback.
        match read_packet(unsafe { sample.as_ref() }, shared) {
            Ok(p) => Output::Packet(p),
            Err(e) => Output::Failed(e),
        }
    }));
    let o = result.unwrap_or_else(|_| Output::Failed("panic while reading a compressed frame".into()));
    shared.out.lock().unwrap_or_else(PoisonError::into_inner).push(o);
}

/// Unlocks a pixel buffer's base address when dropped.
struct Locked<'a>(&'a CVImageBuffer);

impl Drop for Locked<'_> {
    fn drop(&mut self) {
        // SAFETY: the fill functions locked the buffer with the same flags, and it is still alive.
        unsafe { CVPixelBufferUnlockBaseAddress(self.0, CVPixelBufferLockFlags(0)) };
    }
}

fn lock(pb: &CVImageBuffer) -> Result<Locked<'_>, String> {
    // SAFETY: `pb` is a valid pixel buffer (the caller holds it retained).
    if unsafe { CVPixelBufferLockBaseAddress(pb, CVPixelBufferLockFlags(0)) } != 0 {
        return Err("cannot lock the input buffer".into());
    }
    Ok(Locked(pb))
}

/// One writable plane of a locked biplanar buffer: bytes, stride, width, height.
fn plane_mut<'l>(pb: &CVImageBuffer, _lock: &'l mut Locked<'_>, i: usize) -> Result<(&'l mut [u8], usize, usize, usize), String> {
    let base = CVPixelBufferGetBaseAddressOfPlane(pb, i) as *mut u8;
    let stride = CVPixelBufferGetBytesPerRowOfPlane(pb, i);
    let (width, height) = (CVPixelBufferGetWidthOfPlane(pb, i), CVPixelBufferGetHeightOfPlane(pb, i));
    if base.is_null() || stride == 0 || height == 0 {
        return Err(format!("plane {i} of the input buffer is not mapped"));
    }
    let len = stride.checked_mul(height).ok_or("plane size overflows")?;
    // SAFETY: the base address is locked for writing while `_lock` lives and the slice borrows it;
    // CoreVideo maps `bytes_per_row * height` bytes from each plane's base address.
    Ok((unsafe { std::slice::from_raw_parts_mut(base, len) }, stride, width, height))
}

/// Copy 10-bit planar Y'CbCr (code values, `cw` chroma samples per row, `h` rows each) into an
/// `x422` / `x444` buffer: 16-bit samples with the value in the top ten bits, chroma interleaved.
fn fill_biplanar_10(pb: &CVImageBuffer, w: usize, h: usize, cw: usize, y: &[u16], u: &[u16], v: &[u16]) -> Result<(), String> {
    if y.len() < w * h || u.len() < cw * h || v.len() < cw * h {
        return Err("the Y'CbCr planes are smaller than the picture".into());
    }
    let mut lock = lock(pb)?;
    let (luma, ls, lw, lh) = plane_mut(pb, &mut lock, 0)?;
    if lw < w || lh < h || ls < w * 2 {
        return Err("the input buffer's luma plane is smaller than the picture".into());
    }
    for (row, src) in luma.chunks_mut(ls).zip(y.chunks_exact(w)).take(h) {
        for (dst, &s) in row.as_chunks_mut::<2>().0.iter_mut().zip(src) {
            *dst = (s.min(1023) << 6).to_ne_bytes();
        }
    }
    let (chroma, cs, cpw, cph) = plane_mut(pb, &mut lock, 1)?;
    if cpw < cw || cph < h || cs < cw * 4 {
        return Err("the input buffer's chroma plane is smaller than the picture".into());
    }
    for ((row, us), vs) in chroma.chunks_mut(cs).zip(u.chunks_exact(cw)).zip(v.chunks_exact(cw)).take(h) {
        for ((quad, &cb), &cr) in row.as_chunks_mut::<4>().0.iter_mut().zip(us).zip(vs) {
            let (b, r) = ((cb.min(1023) << 6).to_ne_bytes(), (cr.min(1023) << 6).to_ne_bytes());
            *quad = [b[0], b[1], r[0], r[1]];
        }
    }
    Ok(())
}

/// 12 bits to 16 by bit replication (0 and 4095 stay at the ends of the range).
fn to16(v: u16) -> u16 {
    let v = v.min(4095);
    (v << 4) | (v >> 8)
}

/// Copy 12-bit planar 4:4:4 Y'CbCr and alpha into a packed `y416` buffer (A, Y', Cb, Cr per pixel,
/// little-endian 16-bit).
fn fill_y416(pb: &CVImageBuffer, w: usize, h: usize, y: &[u16], u: &[u16], v: &[u16], a: &[u16]) -> Result<(), String> {
    let n = w * h;
    if y.len() < n || u.len() < n || v.len() < n || a.len() < n {
        return Err("the Y'CbCr / alpha planes are smaller than the picture".into());
    }
    let _lock = lock(pb)?;
    let base = CVPixelBufferGetBaseAddress(pb) as *mut u8;
    let stride = CVPixelBufferGetBytesPerRow(pb);
    if base.is_null() || CVPixelBufferGetWidth(pb) < w || CVPixelBufferGetHeight(pb) < h || stride < w * 8 {
        return Err("the input buffer is smaller than the picture".into());
    }
    let len = stride.checked_mul(h).ok_or("picture size overflows")?;
    // SAFETY: the base address is locked for writing until `_lock` drops, after the last use of
    // `data`; CoreVideo maps `bytes_per_row * height` bytes from it for a non-planar buffer.
    let data = unsafe { std::slice::from_raw_parts_mut(base, len) };
    for (row, k) in data.chunks_mut(stride).zip((0..h).map(|r| r * w)).take(h) {
        for (x, px) in row.as_chunks_mut::<8>().0.iter_mut().take(w).enumerate() {
            let i = k + x;
            let s = [to16(a[i]), to16(y[i]), to16(u[i]), to16(v[i])].map(u16::to_le_bytes);
            *px = [s[0][0], s[0][1], s[1][0], s[1][1], s[2][0], s[2][1], s[3][0], s[3][1]];
        }
    }
    Ok(())
}

fn set_value(session: &VTCompressionSession, key: &CFString, value: &CFType) -> i32 {
    // SAFETY: the session is valid, `key` is a framework constant and `value` a live CF object.
    unsafe { VTSessionSetProperty(session.as_ref(), key, Some(value)) }
}

fn require(status: i32, what: &str) -> Result<(), String> {
    if status == 0 { Ok(()) } else { Err(format!("VideoToolbox refused {what} ({status})")) }
}

/// A hardware ProRes encoder: feed pictures, collect compressed frames in presentation order.
pub struct VtProResEncoder {
    session: Session,
    config: ProResConfig,
    pool: Option<CFRetained<CVPixelBufferPool>>,
    attrs: CFRetained<CFDictionary<CFString, CFType>>,
}

// SAFETY: the session and its callback state are `Send` (see `Session`); the attribute dictionary
// is never mutated after creation, CoreFoundation dictionaries and CoreVideo pools are thread-safe,
// and the encoder is used by one thread at a time (`&mut self`).
unsafe impl Send for VtProResEncoder {}

impl VtProResEncoder {
    /// Create the session, or say why hardware ProRes encoding is not used for this configuration.
    pub fn new(config: ProResConfig) -> Result<VtProResEncoder, String> {
        let (w, h) = (config.width, config.height);
        if w == 0 || h == 0 || w > MAX_SIDE || h > MAX_SIDE {
            return Err(format!("picture size {w}x{h}"));
        }
        if !matches!(&config.fourcc, b"apco" | b"apcs" | b"apcn" | b"apch" | b"ap4h" | b"ap4x") {
            return Err(format!("{} is not a ProRes profile", String::from_utf8_lossy(&config.fourcc)));
        }
        if config.alpha && !config.is_4444() {
            return Err("alpha needs ProRes 4444".into());
        }
        if config.timescale == 0 || config.frame_duration == 0 || config.timescale > i32::MAX as u32 {
            return Err("invalid frame rate".into());
        }
        let shared = Box::new(Shared { timescale: i64::from(config.timescale), out: Mutex::new(Vec::new()) });
        // SAFETY: reading immutable framework constants.
        let require_key: &CFString = unsafe { kVTVideoEncoderSpecification_RequireHardwareAcceleratedVideoEncoder };
        let spec = CFDictionary::<CFString, CFType>::from_slices(&[require_key], &[CFBoolean::new(true).as_ref()]);
        let (pf, cw, ch) = (CFNumber::new_i32(config.pixel_format() as i32), CFNumber::new_i32(w as i32), CFNumber::new_i32(h as i32));
        let io = CFDictionary::<CFString, CFType>::from_slices(&[], &[]);
        // SAFETY: reading immutable framework constants.
        let (pf_key, w_key, h_key, io_key): (&CFString, &CFString, &CFString, &CFString) =
            unsafe { (kCVPixelBufferPixelFormatTypeKey, kCVPixelBufferWidthKey, kCVPixelBufferHeightKey, kCVPixelBufferIOSurfacePropertiesKey) };
        let attrs = CFDictionary::<CFString, CFType>::from_slices(&[pf_key, w_key, h_key, io_key], &[pf.as_ref(), cw.as_ref(), ch.as_ref(), io.as_ref()]);
        let mut out: *mut VTCompressionSession = std::ptr::null_mut();
        // SAFETY: every pointer is valid for the call (VideoToolbox copies what it keeps); the
        // callback's refcon points into `shared`, which the returned `Session` keeps alive until
        // the session is invalidated.
        let status = unsafe {
            VTCompressionSession::create(
                None,
                w as i32,
                h as i32,
                u32::from_be_bytes(config.fourcc),
                Some(spec.as_ref()),
                Some(attrs.as_ref()),
                None,
                Some(output_callback),
                &*shared as *const Shared as *mut c_void,
                NonNull::from(&mut out),
            )
        };
        let session = NonNull::new(out).filter(|_| status == 0).ok_or_else(|| format!("VTCompressionSessionCreate failed ({status})"))?;
        // SAFETY: a created session is returned retained (+1); `CFRetained` takes that reference over.
        let session = Session { session: unsafe { CFRetained::from_raw(session) }, shared };
        Self::configure(&session.session, &config)?;
        // SAFETY: the session is valid. Preparing allocates the encoder now, so a configuration
        // the hardware cannot take fails here and not on the first frame.
        require(unsafe { session.session.prepare_to_encode_frames() }, "to prepare the encoder")?;
        // SAFETY: the session is valid; the pool (if any) is returned retained.
        let pool = unsafe { session.session.pixel_buffer_pool() };
        Ok(VtProResEncoder { session, config, pool, attrs })
    }

    fn configure(session: &VTCompressionSession, c: &ProResConfig) -> Result<(), String> {
        // SAFETY: reading immutable framework constants.
        let (realtime, fps_key) = unsafe { (kVTCompressionPropertyKey_RealTime, kVTCompressionPropertyKey_ExpectedFrameRate) };
        require(set_value(session, realtime, CFBoolean::new(false).as_ref()), "offline encoding")?;
        let _ = set_value(session, fps_key, CFNumber::new_f64(f64::from(c.timescale) / f64::from(c.frame_duration)).as_ref());
        // The colour written to every frame header (and to the pixel buffers' attachments):
        // BT.709 for SDR, BT.2020 with PQ or HLG for HDR, as the built-in encoder writes it.
        // SAFETY: reading immutable framework constants.
        let (pk, tk, mk): (&CFString, &CFString, &CFString) =
            unsafe { (kVTCompressionPropertyKey_ColorPrimaries, kVTCompressionPropertyKey_TransferFunction, kVTCompressionPropertyKey_YCbCrMatrix) };
        // SAFETY: reading immutable framework constants.
        let (pv, tv, mv): (&CFString, &CFString, &CFString) = unsafe {
            match c.signal {
                ProResSignal::Bt709 => {
                    (kCVImageBufferColorPrimaries_ITU_R_709_2, kCVImageBufferTransferFunction_ITU_R_709_2, kCVImageBufferYCbCrMatrix_ITU_R_709_2)
                }
                ProResSignal::Pq => {
                    (kCVImageBufferColorPrimaries_ITU_R_2020, kCVImageBufferTransferFunction_SMPTE_ST_2084_PQ, kCVImageBufferYCbCrMatrix_ITU_R_2020)
                }
                ProResSignal::Hlg => {
                    (kCVImageBufferColorPrimaries_ITU_R_2020, kCVImageBufferTransferFunction_ITU_R_2100_HLG, kCVImageBufferYCbCrMatrix_ITU_R_2020)
                }
            }
        };
        require(set_value(session, pk, pv.as_ref()), "the colour primaries")?;
        require(set_value(session, tk, tv.as_ref()), "the transfer function")?;
        require(set_value(session, mk, mv.as_ref()), "the Y'CbCr matrix")?;
        Ok(())
    }

    /// Whether VideoToolbox says this session runs on a hardware encoder (it is created with one
    /// required; asking means a dropped requirement could never turn into a silent software encode).
    pub fn uses_hardware(&self) -> bool {
        // SAFETY: reading an immutable framework constant.
        let key: &CFString = unsafe { kVTCompressionPropertyKey_UsingHardwareAcceleratedVideoEncoder };
        let mut value: *mut CFType = std::ptr::null_mut();
        // SAFETY: the session is valid and `value` is a valid out-pointer for the CFTypeRef the
        // property is copied to (returned retained, +1); it stays null when the call fails.
        let status = unsafe { VTSessionCopyProperty(self.session.session.as_ref(), key, None, (&raw mut value).cast()) };
        let Some(value) = NonNull::new(value).filter(|_| status == 0) else { return false };
        // SAFETY: a copied property is returned retained (+1); `CFRetained` takes that reference over.
        let value = unsafe { CFRetained::from_raw(value) };
        value.downcast_ref::<CFBoolean>().is_some_and(CFBoolean::as_bool)
    }

    pub fn config(&self) -> &ProResConfig {
        &self.config
    }

    fn new_buffer(&self) -> Result<CFRetained<CVImageBuffer>, String> {
        let mut raw: *mut CVImageBuffer = std::ptr::null_mut();
        let status = match &self.pool {
            // SAFETY: the pool is valid and `raw` is a valid out-pointer.
            Some(pool) => unsafe { CVPixelBufferPool::create_pixel_buffer(None, pool, NonNull::from(&mut raw)) },
            // SAFETY: `attrs` is a valid dictionary of CF types and `raw` a valid out-pointer.
            None => unsafe {
                CVPixelBufferCreate(
                    None,
                    self.config.width as usize,
                    self.config.height as usize,
                    self.config.pixel_format(),
                    Some(self.attrs.as_ref()),
                    NonNull::from(&mut raw),
                )
            },
        };
        let raw = NonNull::new(raw).filter(|_| status == 0).ok_or_else(|| format!("cannot allocate an input buffer ({status})"))?;
        // SAFETY: both creation functions return the buffer retained (+1).
        Ok(unsafe { CFRetained::from_raw(raw) })
    }

    /// Submit 422 picture number `index`: 10-bit 4:2:2 planar Y'CbCr code values, tightly packed.
    pub fn encode_422(&mut self, index: u64, y: &[u16], u: &[u16], v: &[u16]) -> Result<Vec<ProResPacket>, String> {
        if self.config.is_4444() {
            return Err("a 4:2:2 picture for a 4444 encoder".into());
        }
        let (w, h) = (self.config.width as usize, self.config.height as usize);
        let buffer = self.new_buffer()?;
        fill_biplanar_10(&buffer, w, h, w.div_ceil(2), y, u, v)?;
        self.submit(index, &buffer)
    }

    /// Submit 4444 picture number `index`: 12-bit 4:4:4 planar Y'CbCr code values and, for an
    /// alpha session, the 12-bit alpha plane.
    pub fn encode_4444(&mut self, index: u64, y: &[u16], u: &[u16], v: &[u16], alpha: Option<&[u16]>) -> Result<Vec<ProResPacket>, String> {
        if !self.config.is_4444() {
            return Err("a 4:4:4 picture for a 422 encoder".into());
        }
        let (w, h) = (self.config.width as usize, self.config.height as usize);
        let buffer = self.new_buffer()?;
        match (self.config.alpha, alpha) {
            (true, Some(a)) => fill_y416(&buffer, w, h, y, u, v, a)?,
            (true, None) => return Err("an alpha session without an alpha plane".into()),
            (false, _) => {
                // 12 bits to 10, rounded
                let ten = |p: &[u16]| p.iter().map(|&s| ((u32::from(s) + 2) >> 2).min(1023) as u16).collect::<Vec<u16>>();
                fill_biplanar_10(&buffer, w, h, w, &ten(y), &ten(u), &ten(v))?
            }
        }
        self.submit(index, &buffer)
    }

    fn submit(&mut self, index: u64, buffer: &CVImageBuffer) -> Result<Vec<ProResPacket>, String> {
        let value = i64::try_from(index).ok().and_then(|i| i.checked_mul(i64::from(self.config.frame_duration))).ok_or("frame index out of range")?;
        let (pts, duration) = (cm_time(value, self.config.timescale), cm_time(i64::from(self.config.frame_duration), self.config.timescale));
        // SAFETY: the session and buffer are valid (VideoToolbox retains the buffer as long as it
        // needs it); no frame properties, refcon or info-flags out-pointer.
        let status = unsafe { self.session.session.encode_frame(buffer, pts, duration, None, std::ptr::null_mut(), std::ptr::null_mut()) };
        if status != 0 {
            return Err(format!("VTCompressionSessionEncodeFrame failed ({status})"));
        }
        self.drain()
    }

    /// Finish every pending frame and return them.
    pub fn flush(&mut self) -> Result<Vec<ProResPacket>, String> {
        // SAFETY: the session is valid; an invalid time means "all pending frames".
        require(unsafe { self.session.session.complete_frames(invalid_time()) }, "to finish the pending frames")?;
        self.drain()
    }

    fn drain(&mut self) -> Result<Vec<ProResPacket>, String> {
        let outputs = std::mem::take(&mut *self.session.shared.out.lock().unwrap_or_else(PoisonError::into_inner));
        let mut packets = Vec::with_capacity(outputs.len());
        for o in outputs {
            match o {
                Output::Packet(p) => packets.push(p),
                Output::Failed(why) => return Err(why),
            }
        }
        packets.sort_by_key(|p| p.pts);
        Ok(packets)
    }
}

/// Whether this Mac has a hardware ProRes encoder for `fourcc` (a small session is created, which
/// takes a few milliseconds).
pub fn prores_encoder_available(fourcc: [u8; 4]) -> bool {
    let config = ProResConfig { width: 256, height: 144, timescale: 24, frame_duration: 1, fourcc, alpha: false, signal: ProResSignal::Bt709 };
    VtProResEncoder::new(config).is_ok_and(|e| e.uses_hardware())
}
