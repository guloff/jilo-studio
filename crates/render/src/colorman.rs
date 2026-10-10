//! Colour management in the renderer: source colour space resolution (Interpret Footage override
//! or stream metadata), source → working conversion of decoded frames, and the working → monitor
//! transform. The maths lives in `filmcraft_color::transform`.
//!
//! A sequence whose pipeline is "plain" (Rec. 709 working space without wide gamut) and media
//! whose metadata says Rec. 709 / sRGB / linear take the original fast path unchanged.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use filmcraft_color::{ColorPipeline, ColorSpace, InputTransform, OutputTransform, Range};
use filmcraft_frame::VideoFrame;
use filmcraft_project::{ItemId, Project};

use crate::Image;

/// The Interpret Footage colour-space override of an item (following subclips to their media).
pub fn override_of(project: &Project, item: ItemId) -> Option<ColorSpace> {
    project.resolve_media(item).and_then(|(_, media, _)| media.interpret.color_space)
}

/// The colour space a frame of `item` is interpreted in.
pub fn source_space(project: &Project, item: ItemId, frame: &VideoFrame) -> ColorSpace {
    override_of(project, item).unwrap_or_else(|| ColorSpace::from_info(&frame.color))
}

/// Whether the frame needs more than the default (transfer-only) decode.
pub fn needs_management(pipe: &ColorPipeline, cs: ColorSpace, frame: &VideoFrame) -> bool {
    let default_decode = cs == ColorSpace::from_info(&frame.color) && matches!(cs, ColorSpace::Rec709 | ColorSpace::Srgb | ColorSpace::Linear709);
    !(pipe.is_plain() && default_decode)
}

type Key = (ColorSpace, Range, ColorPipeline, Option<u32>);

/// The content peak (cd/m²) of an item's HDR metadata (MaxCLL, else the mastering display peak),
/// following subclips to their media. Used as the tone-mapping source peak of PQ media.
pub fn source_peak_nits(project: &Project, item: ItemId) -> Option<f32> {
    project.resolve_media(item).and_then(|(_, media, _)| media.info.video.as_ref()?.hdr.as_ref()?.peak_nits())
}

/// A cached input transform (`peak_nits`: the PQ content peak from the file's HDR metadata).
pub fn input_transform(cs: ColorSpace, range: Range, pipe: &ColorPipeline, peak_nits: Option<f32>) -> Arc<InputTransform> {
    static C: OnceLock<Mutex<HashMap<Key, Arc<InputTransform>>>> = OnceLock::new();
    let c = C.get_or_init(Default::default);
    let key = (cs, range, *pipe, peak_nits.map(|p| p.round() as u32));
    if let Some(t) = c.lock().unwrap_or_else(|e| e.into_inner()).get(&key) {
        return t.clone();
    }
    let t = Arc::new(InputTransform::new(cs, range, pipe, key.3.map(f64::from)));
    c.lock().unwrap_or_else(|e| e.into_inner()).insert(key, t.clone());
    t
}

/// Decode a frame of `item` (box-decimated by `n`) into the working space of `pipe`.
pub fn decode(project: &Project, item: ItemId, frame: &VideoFrame, n: usize, pipe: &ColorPipeline) -> Image {
    let cs = source_space(project, item, frame);
    if !needs_management(pipe, cs, frame) {
        let (w, h, px) = frame.to_linear_f32_decimated(n);
        return Image { w, h, px };
    }
    // RGB stills/generators carry full-range code values; YUV carries its own range flag
    let range = match frame.data {
        filmcraft_frame::PixelData::Rgba8(_) | filmcraft_frame::PixelData::RgbaF32(_) => Range::Full,
        _ => frame.color.range,
    };
    let t = input_transform(cs, range, pipe, source_peak_nits(project, item));
    let float_src = matches!(frame.data, filmcraft_frame::PixelData::RgbaF32(_));
    // float frames are linear already: only the gamut/tone stages apply to them
    let (w, h, px) = frame.to_linear_f32_decimated_with(n, if float_src { None } else { Some(&t.table) });
    let mut img = Image { w, h, px };
    if t.has_pixel_stage() {
        img.map_rgb(|c, _, _| t.apply(c));
    }
    img
}

/// Working space → monitor (SDR BT.709) for a non-plain pipeline; no-op otherwise.
pub fn to_display(img: &mut Image, pipe: &ColorPipeline) {
    if pipe.is_plain() {
        return;
    }
    let o = display_transform(pipe);
    if o.is_identity() {
        return;
    }
    img.map_rgb(|c, _, _| o.apply(c));
}

/// How the monitors show HDR on an extended-dynamic-range display (macOS EDR).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Edr {
    /// Working units (1.0 = 203 cd/m²) → display units (1.0 = SDR white): 203 / HDR graphics white.
    pub white_scale: f32,
    /// The largest value the display shows now (≥ 1; e.g. 2–16 on an XDR display).
    pub headroom: f32,
}

impl Edr {
    /// `graphics_white_nits`: the Settings ▸ Color ▸ HDR Graphics White (100, 203 or 300).
    pub fn new(graphics_white_nits: f32, headroom: f32) -> Edr {
        let white = if graphics_white_nits.is_finite() && graphics_white_nits > 0.0 { graphics_white_nits } else { 203.0 };
        let headroom = if headroom.is_finite() { headroom.max(1.0) } else { 1.0 };
        Edr { white_scale: (filmcraft_color::REFERENCE_WHITE_NITS as f32) / white, headroom }
    }
}

/// A rendered working-space image (premultiplied, over black) → half-float RGBA for an EDR monitor:
/// BT.709 linear with SDR white at 1.0 and, for an HDR pipeline, highlights above 1.0 rolled off
/// towards the display's headroom; encoded with the (extended) sRGB curve, as the monitor surface
/// (extended sRGB) expects. An SDR pipeline stays within 0–1.
pub fn to_edr(img: &Image, pipe: &ColorPipeline, edr: Edr) -> Vec<u16> {
    use rayon::prelude::*;
    let hdr = pipe.working.is_hdr();
    let o = OutputTransform::display_edr(pipe);
    let identity = o.is_identity();
    let (top, knee) = if hdr { (edr.headroom, 0.75 * edr.headroom) } else { (1.0, 1.0) };
    let shoulder = |c: [f32; 3]| -> [f32; 3] {
        let m = c[0].max(c[1]).max(c[2]);
        if m <= knee || m <= 0.0 {
            return c;
        }
        let span = (top - knee).max(1e-6);
        let to = knee + span * (1.0 - (-(m - knee) / span).exp());
        c.map(|v| v * to / m)
    };
    let enc = |v: f32| -> u16 {
        let a = v.abs();
        let e = if a <= 0.003_130_8 { 12.92 * a } else { 1.055 * a.powf(1.0 / 2.4) - 0.055 };
        half::f16::from_f32(e.copysign(v)).to_bits()
    };
    let mut out = vec![0u16; img.w * img.h * 4];
    out.par_chunks_mut(img.w.max(1) * 4).zip(img.px.par_chunks(img.w.max(1) * 4)).for_each(|(o_row, s_row)| {
        for (d, s) in o_row.as_chunks_mut::<4>().0.iter_mut().zip(s_row.as_chunks::<4>().0) {
            let mut c = [s[0], s[1], s[2]];
            if !identity {
                c = o.apply(c);
            }
            if hdr {
                c = c.map(|v| v * edr.white_scale);
            }
            let c = shoulder(c).map(|v| v.min(top));
            *d = [enc(c[0]), enc(c[1]), enc(c[2]), half::f16::ONE.to_bits()];
        }
    });
    out
}

pub fn display_transform(pipe: &ColorPipeline) -> Arc<OutputTransform> {
    static C: OnceLock<Mutex<HashMap<ColorPipeline, Arc<OutputTransform>>>> = OnceLock::new();
    let c = C.get_or_init(Default::default);
    c.lock().unwrap_or_else(|e| e.into_inner()).entry(*pipe).or_insert_with(|| Arc::new(OutputTransform::display(pipe))).clone()
}

#[cfg(test)]
mod tests {
    use super::*;
    use filmcraft_color::{ColorInfo, Primaries, Transfer, WorkingSpace};

    fn f16(bits: u16) -> f32 {
        half::f16::from_bits(bits).to_f32()
    }

    #[test]
    fn edr_keeps_sdr_white_at_one_and_lifts_hdr_highlights_up_to_the_headroom() {
        let pq = ColorPipeline { working: WorkingSpace::Rec2100Pq, wide_gamut: false, auto_tone_map: true };
        // working units: 1.0 = 203 cd/m² (graphics white), 4.93 ≈ 1000 cd/m²
        let img = Image { w: 4, h: 1, px: vec![1.0, 1.0, 1.0, 1.0, 4.93, 4.93, 4.93, 1.0, 100.0, 100.0, 100.0, 1.0, 0.18, 0.18, 0.18, 1.0] };
        let out = to_edr(&img, &pq, Edr::new(203.0, 8.0));
        let px = |i: usize| [f16(out[4 * i]), f16(out[4 * i + 1]), f16(out[4 * i + 2]), f16(out[4 * i + 3])];
        assert!((px(0)[0] - 1.0).abs() < 2e-3, "graphics white is SDR white: {:?}", px(0));
        let enc = |v: f32| 1.055 * v.powf(1.0 / 2.4) - 0.055;
        assert!((px(1)[1] - enc(4.93)).abs() < 0.02, "1000 cd/m² passes below the knee: {:?}", px(1));
        assert!(px(2)[2] <= enc(8.0) + 1e-3 && px(2)[2] > enc(7.0), "the headroom bounds the peak: {:?}", px(2));
        assert_eq!(px(0)[3], 1.0);
        // graphics white 100 cd/m²: everything twice as bright relative to SDR white
        let brighter = to_edr(&img, &pq, Edr::new(100.0, 8.0));
        assert!(f16(brighter[0]) > enc(1.9) && f16(brighter[0]) < enc(2.1));
        // SDR pipeline: clipped to the SDR range, with the same curve as the 8-bit monitors
        let sdr = to_edr(&img, &ColorPipeline::REC709, Edr::new(203.0, 8.0));
        assert!((f16(sdr[4]) - 1.0).abs() < 2e-3 && (f16(sdr[12]) - filmcraft_color::linear_to_srgb(0.18)).abs() < 2e-3);
        // a display without headroom never goes past 1.0; hostile parameters are sanitised
        let flat = to_edr(&img, &pq, Edr::new(f32::NAN, f32::NAN));
        assert!(flat.iter().all(|b| f16(*b) <= 1.0 + 1e-3));
    }

    fn frame(rgb: [u8; 3], color: ColorInfo) -> VideoFrame {
        let mut f = VideoFrame::rgba8(2, 2, [rgb[0], rgb[1], rgb[2], 255].repeat(4));
        f.color = color;
        f
    }

    #[test]
    fn mastering_metadata_sets_the_tone_mapping_peak() {
        use filmcraft_color::HdrMetadata;
        use filmcraft_media::MediaSource;
        use filmcraft_project::{ItemKind, Label, MediaClip, MediaRef};
        let pq = ColorInfo { transfer: Transfer::Pq, primaries: Primaries::Bt2020, ..ColorInfo::SRGB_FULL };
        // a PQ grey of ≈ 2000 cd/m² (code 0.83) and one of ≈ 4000 cd/m² (code 0.90)
        let px = |nits: f32| (filmcraft_color::pq_inverse_eotf(nits / 10_000.0) * 255.0).round() as u8;
        let item_with = |hdr: Option<HdrMetadata>| {
            let mut p = Project::new("t");
            let g = filmcraft_media::Generator::BlackVideo;
            let mut info = filmcraft_media::generators::GeneratorSource::new(g.clone(), 2, 2, Default::default(), filmcraft_time::Tick(1)).info().clone();
            if let Some(v) = info.video.as_mut() {
                v.color = pq;
                v.hdr = hdr;
            }
            let clip = MediaClip {
                media: MediaRef::Generator(g),
                info,
                interpret: Default::default(),
                mark_in: None,
                mark_out: None,
                markers: vec![],
                offline: false,
                proxy: None,
                identity: None,
            };
            let id = p.add_item("hdr", Label::Iris, ItemKind::Media(clip), None);
            (p, id)
        };
        let out = |hdr: Option<HdrMetadata>, nits: f32| {
            let (p, id) = item_with(hdr);
            decode(&p, id, &frame([px(nits); 3], pq), 1, &ColorPipeline::REC709).px[0]
        };
        // no metadata: a 1000 cd/m² master is assumed, so 2000 and 4000 both clip to SDR white
        assert!((out(None, 2000.0) - 1.0).abs() < 0.01 && (out(None, 4000.0) - 1.0).abs() < 0.01);
        // MaxCLL 4000: the knee spreads up to 4000, so 1000 and 2000 stay below white and 4000 reaches it
        let m4000 = HdrMetadata { max_cll: Some(4000.0), mastering_max_nits: Some(4000.0), ..Default::default() };
        let (a, a2, b) = (out(Some(m4000), 1000.0), out(Some(m4000), 2000.0), out(Some(m4000), 4000.0));
        assert!(a < out(None, 1000.0) - 0.01 && a > 0.8 && a < a2 && a2 < b && (b - 1.0).abs() < 0.02, "{a} {a2} {b}");
        // the mastering peak alone works too; MaxCLL wins when it is lower (the real content peak)
        let md = HdrMetadata { mastering_max_nits: Some(4000.0), ..Default::default() };
        assert_eq!(md.peak_nits(), Some(4000.0));
        let both = HdrMetadata { mastering_max_nits: Some(4000.0), max_cll: Some(1500.0), ..Default::default() };
        assert_eq!(both.peak_nits(), Some(1500.0));
        assert_eq!(HdrMetadata { max_cll: Some(0.0), ..Default::default() }.peak_nits(), None, "0 = unknown");
        // and reference white is no lower peak than 203
        assert_eq!(HdrMetadata { max_cll: Some(80.0), ..Default::default() }.peak_nits(), Some(203.0));
    }

    #[test]
    fn plain_media_keeps_the_fast_path() {
        let f = frame([200, 100, 50], ColorInfo::SRGB_FULL);
        assert!(!needs_management(&ColorPipeline::REC709, ColorSpace::Srgb, &f));
        let hdr = ColorPipeline { working: WorkingSpace::Rec2100Pq, ..ColorPipeline::REC709 };
        assert!(needs_management(&hdr, ColorSpace::Srgb, &f));
        let pq = ColorInfo { transfer: Transfer::Pq, primaries: Primaries::Bt2020, ..ColorInfo::SRGB_FULL };
        assert!(needs_management(&ColorPipeline::REC709, ColorSpace::from_info(&pq), &frame([1, 2, 3], pq)));
    }

    #[test]
    fn pq_frame_is_tone_mapped_into_rec709() {
        let p = Project::new("t");
        let pq = ColorInfo { transfer: Transfer::Pq, primaries: Primaries::Bt2020, ..ColorInfo::SRGB_FULL };
        // ~1000 cd/m² white (PQ 0.75) → SDR peak after BT.2390; 203 cd/m² (PQ 0.58) → ≈ 0.8
        let hi = decode(&p, ItemId(99), &frame([192, 192, 192], pq), 1, &ColorPipeline::REC709);
        assert!((hi.px[0] - 1.0).abs() < 0.03, "{:?}", &hi.px[..4]);
        let rw = decode(&p, ItemId(99), &frame([148, 148, 148], pq), 1, &ColorPipeline::REC709);
        assert!((0.7..0.9).contains(&rw.px[0]), "{:?}", &rw.px[..4]);
        // the same frame in a PQ sequence keeps its HDR values (working units, 1.0 = 203 cd/m²)
        let hdr = ColorPipeline { working: WorkingSpace::Rec2100Pq, ..ColorPipeline::REC709 };
        let w = decode(&p, ItemId(99), &frame([192, 192, 192], pq), 1, &hdr);
        assert!(w.px[0] > 4.0, "{:?}", &w.px[..4]);
        let mut d = w.clone();
        to_display(&mut d, &hdr);
        assert!((d.px[0] - 1.0).abs() < 0.03);
    }
}
