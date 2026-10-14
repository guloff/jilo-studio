//! Animated GIF, APNG and WebP — stickers, reactions, animated emoji — as media: every frame is
//! decoded once (with its transparency) and the animation loops for as long as the clip lasts. Like
//! a still it has no length of its own (the clip's length is free; new clips get the still
//! duration); its info says how long one loop is. A file whose frames would take more memory than
//! [`MAX_BYTES`] opens as its first frame.

use std::io::Cursor;
use std::sync::Arc;

use filmcraft_frame::VideoFrame;
use filmcraft_time::{FrameRate, TICKS_PER_SECOND, Tick};
use image::AnimationDecoder;

use crate::still::{DEFAULT_STILL_DURATION, StillSource};
use crate::{FrameRequest, MediaError, MediaInfo, MediaKind, MediaSource, Result, VideoStreamInfo};

/// Most memory the decoded frames of one animation may take.
pub const MAX_BYTES: usize = 512 * 1024 * 1024;

/// Shortest frame time honoured (browsers treat smaller GIF delays as 100 ms; 20 ms is the
/// fastest a GIF can ask for).
const MIN_DELAY_MS: f64 = 20.0;

pub struct AnimatedSource {
    info: MediaInfo,
    /// Frames and the time (from the loop's start) each one ends.
    frames: Vec<(Tick, Arc<VideoFrame>)>,
    /// Length of one loop.
    loop_len: Tick,
}

impl AnimatedSource {
    /// The frames of one loop.
    pub fn frame_count(&self) -> usize {
        self.frames.len()
    }

    /// Length of one loop.
    pub fn loop_len(&self) -> Tick {
        self.loop_len
    }
}

/// The frames of an animated GIF, APNG or WebP; None when the file holds a single picture.
fn frames(bytes: &[u8]) -> Option<image::ImageResult<image::Frames<'_>>> {
    if bytes.starts_with(b"GIF8") {
        let d = image::codecs::gif::GifDecoder::new(Cursor::new(bytes)).ok()?;
        return Some(Ok(d.into_frames()));
    }
    if bytes.starts_with(b"\x89PNG") {
        let d = image::codecs::png::PngDecoder::new(Cursor::new(bytes)).ok()?;
        if !d.is_apng().ok()? {
            return None;
        }
        return Some(d.apng().map(AnimationDecoder::into_frames));
    }
    if bytes.len() > 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        let d = image::codecs::webp::WebPDecoder::new(Cursor::new(bytes)).ok()?;
        if !d.has_animation() {
            return None;
        }
        return Some(Ok(d.into_frames()));
    }
    None
}

/// Open a picture: animated GIF / APNG / WebP as an animation, anything else (or one too large to
/// hold) as a still.
pub fn open(name: &str, bytes: &[u8]) -> Result<Arc<dyn MediaSource>> {
    let Some(all) = frames(bytes) else { return Ok(Arc::new(StillSource::decode(name, bytes)?)) };
    let all = all.map_err(|e| MediaError::Decode(format!("{name}: {e}")))?;
    let mut out: Vec<(Tick, Arc<VideoFrame>)> = Vec::new();
    let mut t = 0.0f64; // ms
    let mut bytes_used = 0usize;
    let mut has_alpha = false;
    for f in all {
        let f = f.map_err(|e| MediaError::Decode(format!("{name}: {e}")))?;
        let (n, d) = f.delay().numer_denom_ms();
        let ms = (f64::from(n) / f64::from(d.max(1))).max(MIN_DELAY_MS);
        let img = f.into_buffer();
        let (w, h) = img.dimensions();
        bytes_used += (w as usize) * (h as usize) * 4;
        if bytes_used > MAX_BYTES {
            // too much to hold: the first frame, as a still
            return Ok(Arc::new(StillSource::decode(name, bytes)?));
        }
        has_alpha |= img.pixels().any(|p| p.0[3] < 255);
        t += ms;
        out.push((Tick((t / 1000.0 * TICKS_PER_SECOND as f64).round() as i64), Arc::new(VideoFrame::rgba8(w, h, img.into_raw()))));
    }
    if out.len() < 2 {
        return Ok(Arc::new(StillSource::decode(name, bytes)?));
    }
    let loop_len = out.last().map_or(Tick(1), |f| f.0).max(Tick(1));
    let (w, h) = (out[0].1.width, out[0].1.height);
    let fps = (out.len() as f64 / loop_len.seconds()).clamp(1.0, 120.0);
    let codec = name.rsplit('.').next().unwrap_or("image").to_ascii_uppercase();
    let info = MediaInfo {
        name: name.to_string(),
        kind: MediaKind::Still,
        duration: DEFAULT_STILL_DURATION,
        video: Some(VideoStreamInfo {
            width: w,
            height: h,
            frame_rate: FrameRate::from_f64(fps),
            par: (1, 1),
            codec: format!("{codec} (animated, {} frames)", out.len()),
            pixel_format: if has_alpha { "RGBA 8-bit".into() } else { "RGB 8-bit".into() },
            color: filmcraft_color::ColorInfo::SRGB_FULL,
            has_alpha,
            bitrate: None,
            hdr: None,
        }),
        audio_streams: Vec::new(),
        container: "Image".into(),
        start_timecode: None,
        file_size: Some(bytes.len() as u64),
    };
    Ok(Arc::new(AnimatedSource { info, frames: out, loop_len }))
}

impl MediaSource for AnimatedSource {
    fn info(&self) -> &MediaInfo {
        &self.info
    }
    fn video_frame(&self, req: FrameRequest) -> Result<Arc<VideoFrame>> {
        let t = Tick(req.time.0.rem_euclid(self.loop_len.0));
        let i = self.frames.partition_point(|f| f.0 <= t).min(self.frames.len() - 1);
        Ok(self.frames[i].1.clone())
    }
    fn audio(&self, _start: i64, _frames: usize, _sample_rate: u32) -> Result<filmcraft_frame::AudioBuffer> {
        Err(MediaError::NoStream("audio"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A two-frame GIF: red then transparent-blue, 100 ms each.
    fn gif() -> Vec<u8> {
        let mut out = Vec::new();
        {
            let mut enc = image::codecs::gif::GifEncoder::new(&mut out);
            enc.set_repeat(image::codecs::gif::Repeat::Infinite).unwrap();
            for c in [[255u8, 0, 0, 255], [0, 0, 255, 0]] {
                let img = image::RgbaImage::from_pixel(8, 8, image::Rgba(c));
                enc.encode_frame(image::Frame::from_parts(img, 0, 0, image::Delay::from_numer_denom_ms(100, 1))).unwrap();
            }
        }
        out
    }

    #[test]
    fn an_animated_gif_loops_its_frames() {
        let src = open("sticker.gif", &gif()).unwrap();
        let info = src.info();
        assert_eq!(info.kind, MediaKind::Still, "free length, like a still");
        assert!(info.video.as_ref().unwrap().has_alpha);
        let at = |ms: f64| src.video_frame(FrameRequest { time: Tick::from_seconds_f64(ms / 1000.0), scale: 1.0 }).unwrap().to_rgba8()[0..4].to_vec();
        assert_eq!(at(50.0), [255, 0, 0, 255]);
        assert_eq!(at(150.0)[3], 0, "the second frame is transparent");
        assert_eq!(at(250.0), [255, 0, 0, 255], "loops after 200 ms");
        assert_eq!(at(-50.0)[3], 0, "times before the start wrap too");
    }

    #[test]
    fn a_single_picture_stays_a_still() {
        let mut png = Vec::new();
        image::RgbaImage::from_pixel(4, 4, image::Rgba([1, 2, 3, 255])).write_to(&mut Cursor::new(&mut png), image::ImageFormat::Png).unwrap();
        let src = open("one.png", &png).unwrap();
        assert!(!src.info().video.as_ref().unwrap().codec.contains("animated"));
        assert!(open("bad.gif", b"GIF89a").is_err());
    }
}
