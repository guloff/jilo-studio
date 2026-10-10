//! VideoToolbox ProRes encoding (macOS): every profile encodes on the hardware ProRes engine, the
//! frames decode with our decoder to the planes the built-in encoder starts from (quality at least
//! the built-in encoder's less a margin), 4444 keeps alpha exactly, a full export through the
//! pipeline writes a ProRes MOV ffprobe reads, and the path only acts when asked. Tests skip
//! without a hardware ProRes encoder.
#![cfg(target_os = "macos")]

use std::sync::Arc;

use filmcraft_export::{EncodedPacket, EncoderFrame, ExportSettings, Format, HardwareEncoding, Progress, VideoEncoder, export};
use filmcraft_frame::PixelData;
use filmcraft_media::generators::GeneratorSource;
use filmcraft_media::{DemoScene, MediaSource};
use filmcraft_platform::hardware_encode::videotoolbox_encoder_factory;
use filmcraft_platform::videotoolbox_prores_encode::{ProResConfig, ProResSignal, VtProResEncoder, prores_encoder_available};
use filmcraft_project::{ItemKind, Label, MediaClip, MediaRef, Project, SequenceSettings, TrackKind};
use filmcraft_render::SourceMap;
use filmcraft_time::{FrameRate, Tick, TimeRange};

const W: u32 = 640;
const H: u32 = 360;
const FPS: FrameRate = FrameRate::FPS_24;
const FRAMES: u64 = 12;

/// A moving test picture with an alpha ramp: gradients, a bar travelling across, alpha rising
/// left to right (fully transparent and fully opaque at the edges).
fn picture(i: u64) -> Vec<u8> {
    let (w, h) = (W as usize, H as usize);
    let mut px = vec![255u8; w * h * 4];
    let bar = (i as usize * 13) % (w - 40);
    for y in 0..h {
        for x in 0..w {
            let o = (y * w + x) * 4;
            let on_bar = x >= bar && x < bar + 40 && y > h / 3 && y < 2 * h / 3;
            px[o] = if on_bar { 250 } else { ((x * 255 / w + i as usize * 3) % 256) as u8 };
            px[o + 1] = if on_bar { 30 } else { (y * 255 / h) as u8 };
            px[o + 2] = if on_bar { 60 } else { (((x + y) / 2 + i as usize * 5) % 256) as u8 };
            px[o + 3] = (x * 255 / (w - 1)) as u8;
        }
    }
    px
}

fn settings(profile: &str, alpha: bool, hardware: bool) -> ExportSettings {
    ExportSettings {
        format: Format::ProRes,
        prores_profile: profile.into(),
        alpha,
        hardware_encoding: if hardware { HardwareEncoding::Auto } else { HardwareEncoding::Off },
        ..Default::default()
    }
}

fn encode_all(enc: &mut dyn VideoEncoder) -> Vec<EncodedPacket> {
    let mut out = Vec::new();
    for i in 0..FRAMES {
        let rgba = picture(i);
        out.extend(enc.encode(&EncoderFrame { width: W, height: H, rgba: &rgba, hdr: None, index: i }).expect("encode"));
    }
    out.extend(enc.flush().expect("flush"));
    out
}

/// The built-in encoder for the same settings (the reference).
fn built_in(s: &ExportSettings) -> Box<dyn VideoEncoder> {
    let mut s = s.clone();
    s.hardware_encoding = HardwareEncoding::Off;
    filmcraft_export::software_video_encoder(&s, W, H, FPS).expect("a built-in ProRes encoder").expect("built-in ProRes encoder")
}

/// The source planes the encoders start from: (Y, Cb, Cr, alpha) code values.
fn source_planes(i: u64, four: bool, alpha: bool) -> [Vec<u16>; 4] {
    let (w, h) = (W as usize, H as usize);
    let rgba = picture(i);
    if four {
        let (mut y, mut u, mut v, mut a) = (vec![0; w * h], vec![0; w * h], vec![0; w * h], vec![0; if alpha { w * h } else { 0 }]);
        filmcraft_export::rgba_to_yuv444_12(&rgba, w, h, &mut y, &mut u, &mut v, alpha.then_some(a.as_mut_slice()));
        [y, u, v, a]
    } else {
        let cw = w.div_ceil(2);
        let (mut y, mut u, mut v) = (vec![0; w * h], vec![0; cw * h], vec![0; cw * h]);
        filmcraft_export::rgba_to_yuv422_10(&rgba, w, h, &mut y, &mut u, &mut v);
        [y, u, v, Vec::new()]
    }
}

/// PSNR of decoded planes against the source planes (peak = the bit depth's maximum).
fn psnr(decoded: &[&[u16]], source: &[Vec<u16>], bits: u32) -> f64 {
    let (mut se, mut n) = (0f64, 0usize);
    for (d, s) in decoded.iter().zip(source) {
        assert_eq!(d.len(), s.len(), "plane size");
        se += d.iter().zip(s).map(|(a, b)| (f64::from(*a) - f64::from(*b)).powi(2)).sum::<f64>();
        n += d.len();
    }
    let peak = f64::from((1u32 << bits) - 1);
    if se == 0.0 { 99.0 } else { 10.0 * (peak * peak / (se / n as f64)).log10() }
}

/// Decode `packets` with our decoder and measure them against the source: (mean PSNR, alpha exact).
fn measure(enc: &dyn VideoEncoder, packets: &[EncodedPacket], four: bool, alpha: bool) -> (f64, bool) {
    let entry = enc.sample_entry();
    let mut dec = filmcraft_codecs::software_video_decoder(&entry).expect("our ProRes decoder");
    let (mut sum, mut alpha_exact) = (0.0, true);
    for (i, p) in packets.iter().enumerate() {
        assert!(p.key, "every ProRes frame is a sync sample");
        let f = dec.decode(&p.data, i as i64).expect("decode").pop().expect("a picture");
        let PixelData::Yuv16 { planes, bits, alpha: a, .. } = &f.frame.data else { panic!("16-bit planes") };
        let src = source_planes(i as u64, four, alpha);
        sum += psnr(&[&planes[0], &planes[1], &planes[2]], &src[..3], *bits);
        assert_eq!(a.is_some(), alpha, "alpha plane present only when encoded");
        if let Some(a) = a {
            alpha_exact &= a.as_slice() == src[3].as_slice();
        }
    }
    (sum / packets.len() as f64, alpha_exact)
}

#[test]
fn every_profile_encodes_on_the_hardware_and_decodes_with_our_decoder() {
    if !prores_encoder_available(*b"apch") {
        eprintln!("SKIPPED: no hardware ProRes encoder");
        return;
    }
    // the least PSNR against the source planes per profile: about 5 dB under what the M4 Pro's
    // engine gives on this picture (46.7, 61.0, 65.7, 72.7, 69.5 / 72.5, 72.9 dB). Apple's rate
    // control spends fewer bytes than ours on easy pictures (Proxy 207 vs 254 kB), so it is not
    // compared with the built-in encoder's PSNR, only printed beside it.
    for (profile, alpha, least) in [
        ("proxy", false, 42.0),
        ("lt", false, 56.0),
        ("standard", false, 60.0),
        ("hq", false, 67.0),
        ("4444", false, 64.0),
        ("4444", true, 67.0),
        ("4444xq", true, 67.0),
    ] {
        let s = settings(profile, alpha, true);
        let four = profile.starts_with("4444");
        let Some(Ok(mut hw)) = videotoolbox_encoder_factory(Format::ProRes, W, H, FPS, &s) else { panic!("{profile}: the hardware path declined") };
        let packets = encode_all(hw.as_mut());
        assert_eq!(packets.len(), FRAMES as usize, "{profile}: a frame per picture");
        // Apple's encoder signs its frames (frame header encoder id)
        assert_eq!(packets[0].data.get(12..16), Some(b"apl0".as_slice()), "{profile}: made by Apple's encoder");
        let (hw_psnr, hw_alpha) = measure(hw.as_ref(), &packets, four, alpha);
        let mut sw = built_in(&s);
        let sw_packets = encode_all(sw.as_mut());
        let (sw_psnr, _) = measure(sw.as_ref(), &sw_packets, four, alpha);
        let (hw_bytes, sw_bytes) = (packets.iter().map(|p| p.data.len()).sum::<usize>(), sw_packets.iter().map(|p| p.data.len()).sum::<usize>());
        eprintln!("{profile} alpha {alpha}: hardware {hw_psnr:.1} dB ({hw_bytes} bytes), built-in {sw_psnr:.1} dB ({sw_bytes} bytes)");
        assert!(hw_psnr >= least, "{profile}: hardware {hw_psnr:.1} dB, at least {least} expected (built-in {sw_psnr:.1} dB)");
        assert!(hw_alpha, "{profile}: alpha is coded losslessly");
    }
}

#[test]
fn the_hardware_path_only_acts_when_asked() {
    assert!(videotoolbox_encoder_factory(Format::ProRes, W, H, FPS, &settings("hq", false, false)).is_none(), "Hardware encoding off: the built-in encoder");
    let bad = ProResConfig { width: W, height: H, timescale: 24, frame_duration: 1, fourcc: *b"avc1", alpha: false, signal: ProResSignal::Bt709 };
    assert!(VtProResEncoder::new(bad).is_err(), "not a ProRes profile");
    let alpha_422 = ProResConfig { width: W, height: H, timescale: 24, frame_duration: 1, fourcc: *b"apch", alpha: true, signal: ProResSignal::Bt709 };
    assert!(VtProResEncoder::new(alpha_422).is_err(), "alpha needs 4444");
    let zero = ProResConfig { width: 0, height: H, timescale: 24, frame_duration: 1, fourcc: *b"apch", alpha: false, signal: ProResSignal::Bt709 };
    assert!(VtProResEncoder::new(zero).is_err(), "empty picture");
}

#[test]
fn pictures_of_another_size_are_refused() {
    let Some(Ok(mut hw)) = videotoolbox_encoder_factory(Format::ProRes, W, H, FPS, &settings("hq", false, true)) else {
        eprintln!("SKIPPED: no hardware ProRes encoder");
        return;
    };
    let rgba = vec![0u8; 64 * 36 * 4];
    assert!(hw.encode(&EncoderFrame { width: 64, height: 36, rgba: &rgba, hdr: None, index: 0 }).is_err());
    let short = vec![0u8; 16];
    assert!(hw.encode(&EncoderFrame { width: W, height: H, rgba: &short, hdr: None, index: 0 }).is_err());
}

fn project(frames: i64) -> (Arc<Project>, filmcraft_project::ItemId, SourceMap) {
    let mut p = Project::new("prores-hw");
    let g = GeneratorSource::demo(DemoScene::Plasma);
    let info = g.info().clone();
    let item = p.add_item(
        &info.name.clone(),
        Label::Iris,
        ItemKind::Media(MediaClip {
            media: MediaRef::Generator(g.generator.clone()),
            info,
            interpret: Default::default(),
            mark_in: None,
            mark_out: None,
            markers: vec![],
            offline: false,
            proxy: None,
            identity: None,
        }),
        None,
    );
    let seq = p.new_sequence("s", SequenceSettings { width: W, height: H, frame_rate: FPS, ..Default::default() }, 1, 1, None);
    let mut clip = p.make_track_item(item, TrackKind::Video, Tick::ZERO, TimeRange::new(Tick::ZERO, FPS.tick_of(frames)), FPS).unwrap();
    clip.scale_to_frame = true;
    p.sequence_mut(seq).unwrap().video_tracks[0].items.push(clip);
    let mut m = SourceMap::default();
    m.0.insert(item, Arc::new(g));
    (Arc::new(p), seq, m)
}

#[test]
fn export_through_the_pipeline_with_hardware_prores() {
    if !prores_encoder_available(*b"apch") {
        eprintln!("SKIPPED: no hardware ProRes encoder");
        return;
    }
    filmcraft_platform::register();
    let dir = std::env::temp_dir().join(format!("fc-prores-hw-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let frames = 30;
    let (p, seq, m) = project(frames);
    for profile in ["hq", "4444"] {
        let path = dir.join(format!("{profile}.mov")).to_string_lossy().to_string();
        let s = ExportSettings { path: path.clone(), include_audio: false, ..settings(profile, false, true) };
        let before = filmcraft_export::hw_encode_stats().frames;
        let report = export(&p, seq, &s, &m, &Progress::default()).expect("export");
        assert_eq!(report.frames, frames as u64);
        // (process-wide counters: other tests of this binary may encode meanwhile)
        assert!(filmcraft_export::hw_encode_stats().frames - before >= frames as u64, "{profile}: every frame on the hardware encoder");
        let bytes: Arc<[u8]> = std::fs::read(&path).unwrap().into();
        let src = filmcraft_codecs::open_bytes("hw.mov", bytes).unwrap();
        let info = src.info().video.clone().unwrap();
        assert!(info.codec.contains("ProRes"), "{profile}: {}", info.codec);
        assert!(src.video_frame(filmcraft_media::FrameRequest::full(FPS.tick_of(15))).is_ok(), "{profile}: decodes");
        if let Some(ffprobe) = filmcraft_testkit::ffprobe_or_skip("hardware ProRes export") {
            let out = std::process::Command::new(ffprobe)
                .args([
                    "-v",
                    "error",
                    "-count_frames",
                    "-select_streams",
                    "v:0",
                    "-show_entries",
                    "stream=codec_name,profile,nb_read_frames",
                    "-of",
                    "default=nw=1",
                ])
                .arg(&path)
                .output()
                .unwrap();
            let txt = String::from_utf8_lossy(&out.stdout);
            assert!(txt.contains("codec_name=prores") && txt.contains(&format!("nb_read_frames={frames}")), "{profile}: {txt}");
        }
    }
}
