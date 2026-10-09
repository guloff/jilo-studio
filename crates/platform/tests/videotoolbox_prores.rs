//! VideoToolbox ProRes 422 decoding against our ProRes decoder (macOS): every profile (Proxy,
//! LT, 422, HQ), an odd picture size and an interlaced stream give the same pictures (size, format,
//! colour, pts; samples within the IDCTs' rounding: Apple's decoder is up to 4 codes of 1023 from
//! ffmpeg's, ours up to 1, both 0.02-0.04 on average); ProRes 4444 stays with our decoder; a
//! mid-stream failure continues in software; damaged samples never crash or hang. Skips without
//! ffmpeg (fixture generator) or without a hardware ProRes decoder.
#![cfg(target_os = "macos")]

mod common;

use common::*;
use filmcraft_codecs::{DecodedFrame, VideoDecoder};
use filmcraft_frame::PixelData;
use filmcraft_platform::HybridDecoder;
use filmcraft_platform::videotoolbox::VtProResDecoder;

/// (file, ffmpeg arguments): 12 frames each.
const PRORES: &[(&str, &[&str])] = &[
    ("prores_proxy.mov", &["-f", "lavfi", "-i", "testsrc2=s=1280x720:r=24:d=0.5,noise=alls=10:allf=t", "-c:v", "prores_ks", "-profile:v", "0"]),
    ("prores_lt.mov", &["-f", "lavfi", "-i", "testsrc2=s=1280x720:r=24:d=0.5,noise=alls=10:allf=t", "-c:v", "prores_ks", "-profile:v", "1"]),
    ("prores_422.mov", &["-f", "lavfi", "-i", "testsrc2=s=1920x1080:r=24:d=0.5,noise=alls=10:allf=t", "-c:v", "prores_ks", "-profile:v", "2"]),
    ("prores_hq.mov", &["-f", "lavfi", "-i", "testsrc2=s=1920x1080:r=24:d=0.5,noise=alls=10:allf=t", "-c:v", "prores_ks", "-profile:v", "3"]),
    // a size that is not a multiple of the 16-pixel macroblock
    ("prores_odd.mov", &["-f", "lavfi", "-i", "testsrc2=s=1278x718:r=24:d=0.5,noise=alls=10:allf=t", "-c:v", "prores_ks", "-profile:v", "3"]),
    (
        "prores_interlaced.mov",
        &[
            "-f",
            "lavfi",
            "-i",
            "testsrc2=s=1920x1080:r=25:d=0.5,noise=alls=10:allf=t",
            "-vf",
            "tinterlace=interleave_top,fieldorder=tff",
            "-flags",
            "+ildct",
            "-c:v",
            "prores_ks",
            "-profile:v",
            "3",
        ],
    ),
];

/// Largest difference between the two decoders, in 10-bit codes, any sample (the IDCTs round
/// differently; measured: 4), and the largest mean difference (measured: under 0.05).
const MAX_CODE_DIFF: u16 = 6;
const MAX_MEAN_DIFF: f64 = 0.1;

fn stream(ff: &std::path::Path, name: &str) -> Option<Stream> {
    let (_, args) = PRORES.iter().find(|f| f.0 == name)?;
    let mut args = args.to_vec();
    args.extend_from_slice(&["-pix_fmt", "yuv422p10le"]);
    Some(read_stream(&fixture(ff, name, &args)?))
}

/// The hardware decoder for a stream, or `None` (skip) when this Mac has none for ProRes.
fn hardware(s: &Stream) -> Option<Box<dyn VideoDecoder>> {
    match filmcraft_platform::videotoolbox_factory(&s.entry) {
        Some(Ok(d)) => {
            assert_eq!(d.name(), "VideoToolbox ProRes");
            Some(d)
        }
        Some(Err(e)) => panic!("factory error: {e}"),
        None => {
            eprintln!("SKIPPED: no VideoToolbox ProRes decoder");
            None
        }
    }
}

/// Same pictures: pts, size, format, colour, aspect; samples within [`MAX_CODE_DIFF`].
fn assert_close(what: &str, a: &[DecodedFrame], b: &[DecodedFrame]) -> u16 {
    assert_eq!(a.iter().map(|f| f.pts).collect::<Vec<_>>(), b.iter().map(|f| f.pts).collect::<Vec<_>>(), "{what}: pts sequence");
    let (mut worst, mut sum, mut n) = (0u16, 0u64, 0usize);
    for (x, y) in a.iter().zip(b) {
        let (fx, fy) = (&x.frame, &y.frame);
        assert_eq!((fx.width, fx.height, fx.par, fx.color), (fy.width, fy.height, fy.par, fy.color), "{what}: pts {} geometry / colour", x.pts);
        let (PixelData::Yuv16 { planes: p, chroma: c, bits: bx, alpha: ax }, PixelData::Yuv16 { planes: q, chroma: d, bits: by, alpha: ay }) =
            (&fx.data, &fy.data)
        else {
            panic!("{what}: pts {}: not 16-bit planar", x.pts)
        };
        assert_eq!((c, bx, ax.is_some()), (d, by, ay.is_some()), "{what}: chroma / bits / alpha");
        for i in 0..3 {
            assert_eq!(p[i].len(), q[i].len(), "{what}: plane {i} size");
            for (u, v) in p[i].iter().zip(q[i].iter()) {
                let d = u.abs_diff(*v);
                worst = worst.max(d);
                sum += u64::from(d);
            }
            n += p[i].len();
        }
    }
    let mean = sum as f64 / n.max(1) as f64;
    assert!(worst <= MAX_CODE_DIFF && mean <= MAX_MEAN_DIFF, "{what}: samples differ by up to {worst} codes, {mean:.4} on average");
    worst
}

#[test]
fn prores_422_matches_our_decoder() {
    let ff = filmcraft_testkit::require_ffmpeg!();
    for (name, _) in PRORES {
        let Some(s) = stream(&ff, name) else { continue };
        let Some(mut hw) = hardware(&s) else { return };
        let mut sw = filmcraft_codecs::software_video_decoder(&s.entry).unwrap();
        assert!(hw.intra_only() && sw.intra_only());
        let a = decode_all(hw.as_mut(), &s.samples);
        let b = decode_all(sw.as_mut(), &s.samples);
        assert_eq!(a.len(), s.samples.len(), "{name}: a picture per sample");
        let worst = assert_close(name, &a, &b);
        eprintln!("{name}: {} pictures, max difference {worst} codes", a.len());
        // every sample on its own and out of order, as the frame cache decodes intra streams
        for i in (0..s.samples.len()).rev().step_by(3) {
            let (smp, pts) = &s.samples[i];
            let one = hw.decode(smp, *pts).unwrap();
            assert_eq!(one.len(), 1, "{name}: sample {i} gives its picture at once");
            assert_close(&format!("{name} sample {i}"), &one, &b[i..=i]);
        }
        assert_eq!(hw.name(), "VideoToolbox ProRes", "{name}: stayed in hardware");
    }
}

#[test]
fn prores_4444_stays_with_our_decoder() {
    let ff = filmcraft_testkit::require_ffmpeg!();
    let args = ["-f", "lavfi", "-i", "testsrc2=s=640x360:r=24:d=0.2", "-c:v", "prores_ks", "-profile:v", "4", "-pix_fmt", "yuva444p10le"];
    let Some(path) = fixture(&ff, "prores_4444.mov", &args) else { return };
    let s = read_stream(&path);
    assert!(filmcraft_platform::videotoolbox_factory(&s.entry).is_none(), "4444 is not taken");
    assert!(VtProResDecoder::new(*b"ap4h", 640, 360).is_err());
}

#[test]
fn mid_stream_failure_continues_in_software() {
    let ff = filmcraft_testkit::require_ffmpeg!();
    let Some(s) = stream(&ff, "prores_hq.mov") else { return };
    let (w, h) = s.entry.video.as_ref().map(|v| (u32::from(v.width), u32::from(v.height))).unwrap();
    let Ok(mut vt) = VtProResDecoder::new(*b"apch", w, h) else {
        eprintln!("SKIPPED: no VideoToolbox ProRes decoder");
        return;
    };
    vt.fail_after(5);
    let mut hy = HybridDecoder::new(Box::new(vt), s.entry.clone(), filmcraft_codecs::hw::StreamInfo::Intra);
    let a = decode_all(&mut hy, &s.samples);
    assert!(!hy.is_hardware(), "switched to software");
    let mut sw = filmcraft_codecs::software_video_decoder(&s.entry).unwrap();
    let b = decode_all(sw.as_mut(), &s.samples);
    assert_close("after the failure", &a, &b);
}

#[test]
fn damaged_samples_never_crash_or_hang() {
    let ff = filmcraft_testkit::require_ffmpeg!();
    let Some(s) = stream(&ff, "prores_422.mov") else { return };
    let Some(mut hw) = hardware(&s) else { return };
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut seed = 0x1234_5678_9abc_def0u64;
        let (good, pts) = s.samples[0].clone();
        let mut cases: Vec<Vec<u8>> = vec![Vec::new(), good[..good.len() / 2].to_vec(), good[..40].to_vec()];
        for _ in 0..40 {
            let mut d = good.clone();
            for _ in 0..8 {
                let i = (xorshift(&mut seed) as usize) % d.len();
                d[i] ^= 1 << (xorshift(&mut seed) % 8);
            }
            cases.push(d);
        }
        // a frame header claiming another size
        let mut other = good.clone();
        other[8 + 8] ^= 0x01;
        cases.push(other);
        for (k, c) in cases.iter().enumerate() {
            // an error or a picture, never a panic or a hang
            let _ = hw.decode(c, pts + 1 + k as i64);
        }
        // and the stream still decodes afterwards
        let ok = hw.decode(&good, pts + 1000).map(|f| f.len());
        let _ = tx.send(ok);
    });
    let r = rx.recv_timeout(std::time::Duration::from_secs(60)).expect("damaged ProRes samples hung the decoder");
    assert_eq!(r.ok(), Some(1), "a good sample decodes after damaged ones");
}

#[test]
#[ignore = "diagnostic: distance of both decoders from ffmpeg's ProRes decoder"]
fn distance_from_ffmpeg() {
    let ff = filmcraft_testkit::require_ffmpeg!();
    for (name, _) in PRORES {
        let Some(s) = stream(&ff, name) else { continue };
        let Some(mut hw) = hardware(&s) else { return };
        let mut sw = filmcraft_codecs::software_video_decoder(&s.entry).unwrap();
        let a = decode_all(hw.as_mut(), &s.samples[..1]);
        let b = decode_all(sw.as_mut(), &s.samples[..1]);
        let raw = dir().join(format!("{name}.yuv"));
        let st = std::process::Command::new(&ff)
            .args(["-y", "-v", "error", "-i"])
            .arg(dir().join(name))
            .args(["-frames:v", "1", "-f", "rawvideo", "-pix_fmt", "yuv422p10le"])
            .arg(&raw)
            .status()
            .unwrap();
        assert!(st.success());
        let bytes = std::fs::read(&raw).unwrap();
        let reference: Vec<u16> = bytes.as_chunks::<2>().0.iter().map(|b| u16::from_le_bytes(*b)).collect();
        for (who, f) in [("VideoToolbox", &a[0]), ("FilmCraft", &b[0])] {
            let PixelData::Yuv16 { planes, .. } = &f.frame.data else { panic!() };
            let ours: Vec<u16> = planes.iter().flat_map(|p| p.iter().copied()).collect();
            let n = ours.len().min(reference.len());
            let (mut max, mut sum, mut diff) = (0u16, 0u64, 0usize);
            for (x, y) in ours[..n].iter().zip(&reference[..n]) {
                let d = x.abs_diff(*y);
                max = max.max(d);
                sum += u64::from(d);
                diff += usize::from(d > 0);
            }
            eprintln!("{name} {who}: vs ffmpeg max {max}, mean {:.4}, differing {:.2}%", sum as f64 / n as f64, 100.0 * diff as f64 / n as f64);
        }
    }
}
