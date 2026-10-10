//! The VideoToolbox H.264 and HEVC encoders behind `filmcraft_export::VideoEncoder` (macOS; safe code
//! around [`crate::videotoolbox_encode`]).
//!
//! H.264 hardware encoding is opt-in per export (`ExportSettings::hardware_encoding`, `Auto`): its output
//! depends on the machine, while the built-in encoder's is byte-identical everywhere. The factory
//! declines, so the built-in encoder is used, for everything the hardware path does not take: other
//! formats, MXF (Annex B), two-pass VBR, HDR, odd picture sizes, non-square pixels, and any
//! configuration VideoToolbox cannot create a hardware session for. A hardware encoder that fails in
//! the middle of an export reports an error (the export stops); it cannot hand the stream over to
//! another encoder.
//!
//! ProRes (all six profiles) takes the hardware path with the same opt-in as H.264: the ProRes
//! engine of Apple silicon Pro / Max chips encodes the planes the built-in encoder would (so the
//! colours are the same), and any configuration it declines uses the built-in encoder.
//!
//! HEVC (`Format::Hevc`) has no built-in encoder, so choosing the format is the opt-in, the same
//! configurations are declined, and a declined export fails with "encoder not available" instead of
//! falling back; [`hevc_available`] tells the format list whether this machine can do it at all.

use std::sync::{Once, OnceLock};

use filmcraft_export::{BitrateMode, EncodedPacket, EncoderFrame, ExportError, ExportSettings, Format, H264Pass, H264Profile, HardwareEncoding, VideoEncoder};
use filmcraft_isobmff::{AvcConfig, HevcConfig, SampleEntry};
use filmcraft_time::FrameRate;

use crate::videotoolbox_encode::{ParamSets, VtCodec, VtConfig, VtEncoder, VtPacket, VtProfile, VtRate};
use crate::videotoolbox_prores_encode::{ProResConfig, ProResPacket, ProResSignal, VtProResEncoder};

/// The hardware encoder's configuration for an export, or why it is not used for it.
pub fn config_for(w: u32, h: u32, rate: FrameRate, s: &ExportSettings) -> Result<VtConfig, String> {
    let hevc = s.format == Format::Hevc;
    if !hevc && s.hardware_encoding != HardwareEncoding::Auto {
        return Err("hardware encoding is off".into());
    }
    if s.format.is_mxf() {
        return Err("MXF carries an Annex B stream".into());
    }
    if s.bitrate_mode == BitrateMode::Vbr2Pass || !matches!(s.h264_pass, H264Pass::Single) {
        return Err("two-pass encoding".into());
    }
    // HDR: HEVC Main 10, BT.2020 NCL with PQ or HLG (the signal the export resolved); H.264 stays SDR
    let hdr = s.signal.is_hdr();
    if hdr && !hevc {
        return Err("HDR needs H.265".into());
    }
    if hdr && (s.signal.primaries != 9 || s.signal.matrix != 9 || !matches!(s.signal.transfer, 16 | 18)) {
        return Err(format!("the colour signal {:?} (HDR here is BT.2020 with PQ or HLG)", s.signal));
    }
    if !w.is_multiple_of(2) || !h.is_multiple_of(2) {
        return Err("odd picture size".into());
    }
    if s.pixel_aspect.is_some_and(|(n, d)| n != d) {
        return Err("non-square pixels".into());
    }
    let timescale = u32::try_from(rate.num).ok().filter(|n| *n > 0).ok_or("invalid frame rate")?;
    let frame_duration = u32::try_from(rate.den).ok().filter(|d| *d > 0).ok_or("invalid frame rate")?;
    let kbps = s.bitrate_kbps.max(100);
    let max_kbps = s.max_bitrate_kbps.filter(|m| *m >= kbps).unwrap_or(kbps.saturating_mul(3) / 2);
    let fps = f64::from(timescale) / f64::from(frame_duration);
    Ok(VtConfig {
        width: w,
        height: h,
        timescale,
        frame_duration,
        keyframe_interval: s.keyframe_distance.filter(|k| *k > 0).unwrap_or_else(|| (fps * 2.0).round().max(1.0) as u32),
        profile: match (hevc, s.h264_profile) {
            (true, _) if hdr => VtProfile::HevcMain10 { hlg: s.signal.transfer == 18 },
            (true, _) => VtProfile::HevcMain,
            (false, H264Profile::Baseline) => VtProfile::Baseline,
            (false, H264Profile::Main) => VtProfile::Main,
            (false, H264Profile::High) => VtProfile::High,
        },
        rate: match s.bitrate_mode {
            BitrateMode::Cbr => VtRate::Cbr { kbps },
            _ => VtRate::Vbr { target_kbps: kbps, max_kbps },
        },
    })
}

static HEVC_PROBE: OnceLock<bool> = OnceLock::new();

/// Whether this machine has a hardware HEVC encoder (asked once, by creating a small session, which
/// takes about 0.1 s): what makes `Format::Hevc` available. [`warm_hevc_probe`] asks it ahead of the
/// first caller.
pub fn hevc_available() -> bool {
    *HEVC_PROBE.get_or_init(|| {
        let probe = VtConfig {
            width: 640,
            height: 360,
            timescale: 30,
            frame_duration: 1,
            keyframe_interval: 30,
            profile: VtProfile::HevcMain,
            rate: VtRate::Vbr { target_kbps: 2000, max_kbps: 3000 },
        };
        let result = VtEncoder::new(probe);
        if let Err(why) = &result {
            log::info!("no hardware HEVC encoder: {why}");
        }
        result.is_ok()
    })
}

static HEVC_HDR_PROBE: OnceLock<bool> = OnceLock::new();

/// Whether this machine's hardware HEVC encoder takes 10-bit HDR (Main 10, BT.2020 PQ): what lets
/// HDR sequences export as HDR H.265 instead of being tone-mapped to SDR. Asked once.
pub fn hevc_hdr_available() -> bool {
    *HEVC_HDR_PROBE.get_or_init(|| {
        let probe = VtConfig {
            width: 640,
            height: 360,
            timescale: 30,
            frame_duration: 1,
            keyframe_interval: 30,
            profile: VtProfile::HevcMain10 { hlg: false },
            rate: VtRate::Vbr { target_kbps: 2000, max_kbps: 3000 },
        };
        let result = VtEncoder::new(probe);
        if let Err(why) = &result {
            log::info!("no hardware HEVC Main 10 encoder: {why}");
        }
        result.is_ok()
    })
}

/// Whether [`hevc_available`] has its answer yet (diagnostics and tests).
pub fn hevc_probed() -> bool {
    HEVC_PROBE.get().is_some()
}

/// Ask [`hevc_available`] now, on a thread of its own, so that the first draw of the format list does
/// not spend its 0.1 s creating a hardware session on the UI thread. Whoever asks while the thread
/// runs waits for the same answer (no longer than asking first); if the thread cannot start, or the
/// probe panics, the first caller asks, as before. Only the first call starts a thread.
pub fn warm_hevc_probe() {
    static STARTED: Once = Once::new();
    STARTED.call_once(|| {
        let spawned = std::thread::Builder::new().name("filmcraft-hevc-probe".into()).spawn(|| {
            let _ = std::panic::catch_unwind(hevc_available);
        });
        if let Err(why) = spawned {
            log::info!("HEVC probe not started ahead of time: {why}");
        }
    });
}

/// The encoder factory `register` puts in front of the built-in ones: a hardware encoder when the
/// export asks for one (H.264 with `hardware_encoding`; HEVC always) and the OS takes the
/// configuration, `None` (the built-in encoder, if there is one) otherwise.
pub fn videotoolbox_encoder_factory(
    format: Format,
    w: u32,
    h: u32,
    rate: FrameRate,
    s: &ExportSettings,
) -> Option<filmcraft_export::Result<Box<dyn VideoEncoder>>> {
    let name = match format {
        Format::ProRes if s.hardware_encoding == HardwareEncoding::Auto => return prores_factory(w, h, rate, s),
        Format::H264 if s.hardware_encoding == HardwareEncoding::Auto => "H.264",
        Format::Hevc => "HEVC",
        _ => return None,
    };
    let config = match config_for(w, h, rate, s) {
        Ok(c) => c,
        Err(why) => {
            log::info!("hardware {name} encoding declined: {why}");
            filmcraft_export::note_hw_encode_declined();
            return None;
        }
    };
    match VtEncoder::new(config) {
        Ok(vt) => Some(Ok({
            // `perf.stats` and the export report count hardware sessions and frames, as for NVENC
            filmcraft_export::note_hw_encode_session();
            Box::new(HardwareEncoder {
                vt,
                signal: s.signal,
                y: Vec::new(),
                u: Vec::new(),
                v: Vec::new(),
                y16: Vec::new(),
                u16: Vec::new(),
                v16: Vec::new(),
                decoded: 0,
            })
        })),
        Err(why) => {
            log::info!("hardware {name} encoding declined: {why}");
            filmcraft_export::note_hw_encode_declined();
            None
        }
    }
}

/// The hardware ProRes encoder for an export, `None` (the built-in encoder) when declined.
fn prores_factory(w: u32, h: u32, rate: FrameRate, s: &ExportSettings) -> Option<filmcraft_export::Result<Box<dyn VideoEncoder>>> {
    let decline = |why: String| {
        log::info!("hardware ProRes encoding declined: {why}");
        filmcraft_export::note_hw_encode_declined();
        None
    };
    let signal = match (s.signal.is_hdr(), s.signal.transfer) {
        (false, _) => ProResSignal::Bt709,
        (true, 16) if s.signal.primaries == 9 && s.signal.matrix == 9 => ProResSignal::Pq,
        (true, 18) if s.signal.primaries == 9 && s.signal.matrix == 9 => ProResSignal::Hlg,
        (true, _) => return decline(format!("the colour signal {:?}", s.signal)),
    };
    let (Ok(timescale), Ok(frame_duration)) = (u32::try_from(rate.num), u32::try_from(rate.den)) else { return decline("frame rate".into()) };
    let profile = filmcraft_export::prores_profile(&s.prores_profile);
    // alpha only from straight RGBA (an HDR export carries none), as the built-in encoder
    let alpha = s.alpha && filmcraft_export::prores_4444(s) && signal == ProResSignal::Bt709;
    let config = ProResConfig { width: w, height: h, timescale, frame_duration, fourcc: profile.fourcc(), alpha, signal };
    match VtProResEncoder::new(config) {
        Ok(vt) if vt.uses_hardware() => {
            filmcraft_export::note_hw_encode_session();
            Some(Ok(Box::new(HardwareProResEncoder { vt, signal: s.signal, y: Vec::new(), u: Vec::new(), v: Vec::new(), a: Vec::new(), encoded: 0 })))
        }
        Ok(_) => decline("the session is not on the hardware encoder".into()),
        Err(why) => decline(why),
    }
}

/// [`VtProResEncoder`] as a `VideoEncoder`: RGBA (or HDR) pictures in, MOV samples out.
pub struct HardwareProResEncoder {
    vt: VtProResEncoder,
    signal: filmcraft_export::ColorSignal,
    y: Vec<u16>,
    u: Vec<u16>,
    v: Vec<u16>,
    a: Vec<u16>,
    /// Compressed frames returned so far.
    encoded: i64,
}

impl HardwareProResEncoder {
    /// MOV samples from compressed frames: every one a sync sample at `n × duration` (a frame at
    /// another time is an error, never a file with wrong times).
    fn samples(&mut self, packets: Vec<ProResPacket>) -> filmcraft_export::Result<Vec<EncodedPacket>> {
        let duration = self.vt.config().frame_duration;
        let mut out = Vec::with_capacity(packets.len());
        for p in packets {
            let n = self.encoded;
            self.encoded += 1;
            if n.checked_mul(i64::from(duration)) != Some(p.pts) {
                return Err(ExportError::Encode(format!("VideoToolbox returned ProRes frame {n} at time {}", p.pts)));
            }
            out.push(EncodedPacket { data: p.data, key: true, duration, composition_offset: 0 });
        }
        Ok(out)
    }
}

impl VideoEncoder for HardwareProResEncoder {
    fn sample_entry(&self) -> SampleEntry {
        let c = self.vt.config();
        let mut e = SampleEntry::prores(filmcraft_isobmff::FourCc(c.fourcc), c.width.min(u32::from(u16::MAX)) as u16, c.height.min(u32::from(u16::MAX)) as u16);
        self.signal.apply_to(&mut e, true);
        e
    }

    fn timescale(&self) -> u32 {
        self.vt.config().timescale
    }

    fn encode(&mut self, f: &EncoderFrame) -> filmcraft_export::Result<Vec<EncodedPacket>> {
        let c = self.vt.config().clone();
        if (f.width, f.height) != (c.width, c.height) {
            return Err(ExportError::Encode(format!("picture is {}x{}, the encoder was created for {}x{}", f.width, f.height, c.width, c.height)));
        }
        let (w, h) = (f.width as usize, f.height as usize);
        if f.hdr.is_none() && f.rgba.len() < w.saturating_mul(h).saturating_mul(4) {
            return Err(ExportError::Encode("the RGBA picture is smaller than its size".into()));
        }
        if f.hdr.is_some_and(|rgb| rgb.len() < w.saturating_mul(h).saturating_mul(3)) {
            return Err(ExportError::Encode("the HDR picture is smaller than its size".into()));
        }
        let (kr, kb) = self.signal.kr_kb();
        let packets = if c.is_4444() {
            let n = w * h;
            for p in [&mut self.y, &mut self.u, &mut self.v] {
                p.resize(n, 0);
            }
            self.a.resize(if c.alpha { n } else { 0 }, 0);
            let (y, u, v) = (&mut self.y, &mut self.u, &mut self.v);
            let a = (!self.a.is_empty()).then_some(self.a.as_mut_slice());
            filmcraft_export::timed(filmcraft_export::Stage::Convert, || match f.hdr {
                Some(rgb) => filmcraft_export::rgbf_to_yuv444_12(rgb, w, h, kr, kb, y, u, v),
                None => filmcraft_export::rgba_to_yuv444_12(f.rgba, w, h, y, u, v, a),
            });
            let a = c.alpha.then_some(self.a.as_slice());
            self.vt.encode_4444(f.index, &self.y, &self.u, &self.v, a).map_err(ExportError::Encode)?
        } else {
            let cw = w.div_ceil(2);
            self.y.resize(w * h, 0);
            self.u.resize(cw * h, 0);
            self.v.resize(cw * h, 0);
            let (y, u, v) = (&mut self.y, &mut self.u, &mut self.v);
            filmcraft_export::timed(filmcraft_export::Stage::Convert, || match f.hdr {
                Some(rgb) => filmcraft_export::rgbf_to_yuv422_10(rgb, w, h, kr, kb, y, u, v),
                None => filmcraft_export::rgba_to_yuv422_10(f.rgba, w, h, y, u, v),
            });
            self.vt.encode_422(f.index, &self.y, &self.u, &self.v).map_err(ExportError::Encode)?
        };
        filmcraft_export::note_hw_encode_frame();
        self.samples(packets)
    }

    fn flush(&mut self) -> filmcraft_export::Result<Vec<EncodedPacket>> {
        let packets = self.vt.flush().map_err(ExportError::Encode)?;
        self.samples(packets)
    }
}

/// The `hvcC` configuration of an HEVC stream: VideoToolbox's own record when it wrote one.
fn hevc_config(p: &ParamSets) -> Result<HevcConfig, String> {
    let record = p.hvcc.as_deref().ok_or("VideoToolbox wrote no hvcC record for the stream")?;
    let cfg = HevcConfig::parse(record).map_err(|e| format!("unreadable hvcC record: {e}"))?;
    if cfg.vps().is_empty() || cfg.sps().is_empty() || cfg.pps().is_empty() || cfg.length_size != 4 {
        return Err("the hvcC record lacks a parameter set or does not use 4-byte NAL lengths".into());
    }
    Ok(cfg)
}

/// [`VtEncoder`] as a `VideoEncoder`: RGBA pictures in, MP4 samples out.
pub struct HardwareEncoder {
    vt: VtEncoder,
    /// The export's colour signal: BT.709, or BT.2020 PQ / HLG for Main 10 (written as `colr`,
    /// and for PQ `mdcv` / `clli`, in the sample entry).
    signal: filmcraft_export::ColorSignal,
    y: Vec<u8>,
    u: Vec<u8>,
    v: Vec<u8>,
    y16: Vec<u16>,
    u16: Vec<u16>,
    v16: Vec<u16>,
    /// Compressed frames returned so far.
    decoded: i64,
}

impl HardwareEncoder {
    /// MP4 samples from compressed frames. Without frame reordering every frame is presented at
    /// its decode time, so composition offsets are 0; a frame whose time is not `n × duration`
    /// after the first (a dropped or reordered frame) is an error, never a file with wrong times.
    fn samples(&mut self, packets: Vec<VtPacket>) -> filmcraft_export::Result<Vec<EncodedPacket>> {
        let duration = self.vt.config().frame_duration;
        let mut out = Vec::with_capacity(packets.len());
        for p in packets {
            let n = self.decoded;
            self.decoded += 1;
            if n.checked_mul(i64::from(duration)) != Some(p.pts) || p.pts != p.dts {
                return Err(ExportError::Encode(format!(
                    "VideoToolbox returned frame {n} at time {} (decode time {}), expected {}",
                    p.pts,
                    p.dts,
                    n.saturating_mul(i64::from(duration))
                )));
            }
            out.push(EncodedPacket { data: p.data, key: p.key, duration, composition_offset: 0 });
        }
        Ok(out)
    }
}

impl VideoEncoder for HardwareEncoder {
    fn sample_entry(&self) -> SampleEntry {
        let c = self.vt.config();
        let (w, h) = (c.width.min(u32::from(u16::MAX)) as u16, c.height.min(u32::from(u16::MAX)) as u16);
        match (c.profile.codec(), self.vt.parameter_sets()) {
            (VtCodec::H264, p) => {
                let (sps, pps) = p.map(|p| (p.sps, p.pps)).unwrap_or_default();
                SampleEntry::avc(AvcConfig::new(sps, pps, 4), w, h)
            }
            // `encode` refuses to go on without a usable record, so a stream that got this far has one
            (VtCodec::Hevc, p) => {
                let mut e = SampleEntry::hevc(p.as_ref().and_then(|p| hevc_config(p).ok()).unwrap_or_default(), w, h);
                if c.profile.ten_bit() {
                    // HDR: `colr` (BT.2020 + PQ / HLG) and, for PQ, `mdcv` / `clli`, as the NVENC path writes
                    self.signal.apply_to(&mut e, false);
                }
                e
            }
        }
    }

    fn timescale(&self) -> u32 {
        self.vt.config().timescale
    }

    fn encode(&mut self, f: &EncoderFrame) -> filmcraft_export::Result<Vec<EncodedPacket>> {
        let c = self.vt.config();
        let codec = c.profile.codec();
        if (f.width, f.height) != (c.width, c.height) {
            return Err(ExportError::Encode(format!("picture is {}x{}, the encoder was created for {}x{}", f.width, f.height, c.width, c.height)));
        }
        let (w, h) = (f.width as usize, f.height as usize);
        let packets = match (c.profile.ten_bit(), f.hdr) {
            // Main 10 takes the encoded R'G'B' floats: 10-bit limited-range BT.2020 NCL
            (true, Some(rgb)) => {
                let (kr, kb) = self.signal.kr_kb();
                filmcraft_export::timed(filmcraft_export::Stage::Convert, || {
                    filmcraft_export::rgbf_to_yuv420_10(rgb, w, h, kr, kb, &mut self.y16, &mut self.u16, &mut self.v16)
                })?;
                self.vt.encode_10(f.index, &self.y16, &self.u16, &self.v16).map_err(ExportError::Encode)?
            }
            (true, None) => return Err(ExportError::Unsupported("hardware HEVC Main 10 needs the HDR picture (EncoderFrame::hdr)".into())),
            (false, Some(_)) => return Err(ExportError::Unsupported("hardware encoding of 8-bit video does not take HDR pictures".into())),
            (false, None) => {
                if f.rgba.len() < w.saturating_mul(h).saturating_mul(4) {
                    return Err(ExportError::Encode("the RGBA picture is smaller than its size".into()));
                }
                filmcraft_export::timed(filmcraft_export::Stage::Convert, || {
                    filmcraft_export::rgba_to_yuv420_8(f.rgba, w, h, &mut self.y, &mut self.u, &mut self.v)
                });
                self.vt.encode(f.index, &self.y, &self.u, &self.v).map_err(ExportError::Encode)?
            }
        };
        filmcraft_export::note_hw_encode_frame();
        // the muxer needs the parameter sets after the first group of pictures
        if f.index == 0 {
            let params = self.vt.parameter_sets().ok_or_else(|| ExportError::Encode("VideoToolbox returned no parameter sets for the first frame".into()))?;
            if codec == VtCodec::Hevc {
                hevc_config(&params).map_err(ExportError::Encode)?;
            }
        }
        self.samples(packets)
    }

    fn flush(&mut self) -> filmcraft_export::Result<Vec<EncodedPacket>> {
        let packets = self.vt.flush().map_err(ExportError::Encode)?;
        self.samples(packets)
    }
}
