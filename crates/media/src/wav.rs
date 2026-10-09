//! WAV (RIFF / RF64 / BW64; PCM 8/16/24/32-bit int, 32/64-bit float, WAVE_FORMAT_EXTENSIBLE) source,
//! with the Broadcast Wave start time: `bext` TimeReference (EBU Tech 3285), else iXML
//! `BWF_TIME_REFERENCE_LOW/HIGH`.
//!
//! Read in place (#279): opening reads only the chunk headers and the small metadata chunks; the
//! samples are read from the [`ByteReader`] when [`MediaSource::audio`] asks for them. A 90-minute
//! recording no longer costs its size in memory (twice, while it was being opened), and importing
//! it no longer stalls the window while the whole file is read.

use std::sync::Arc;

use filmcraft_frame::{AudioBuffer, VideoFrame};
use filmcraft_time::Tick;

use crate::reader::{MemReader, SharedReader};
use crate::{AudioStreamInfo, FrameRequest, MediaError, MediaInfo, MediaKind, MediaSource, Result, SharedSource};

pub fn sniff(b: &[u8]) -> bool {
    b.len() >= 12 && matches!(&b[0..4], b"RIFF" | b"RF64" | b"BW64") && &b[8..12] == b"WAVE"
}

/// Opens WAV files from a reader without reading the samples (registered with the codecs' reader
/// openers, so imports, the Media Browser and the CLI all stream WAV).
pub fn reader_opener(name: &str, head: &[u8], reader: &SharedReader) -> Option<Result<SharedSource>> {
    sniff(head).then(|| WavSource::open_reader(name, reader.clone()).map(|w| Arc::new(w) as SharedSource))
}

/// Largest metadata chunk read (iXML can carry long take notes; anything larger is skipped).
const MAX_META_CHUNK: u64 = 1 << 20;
/// Most chunks walked before giving up on finding `fmt ` and `data` (a corrupt file can't spin).
const MAX_CHUNKS: usize = 4096;
/// Largest single read of sample bytes; longer requests are read in pieces of this size.
const MAX_READ: usize = 8 << 20;

pub struct WavSource {
    info: MediaInfo,
    reader: SharedReader,
    data_off: u64,
    /// Whole frames in the data chunk.
    frames: u64,
    /// Bytes per frame (`nBlockAlign`, at least channels × container bytes).
    block_align: usize,
    channels: usize,
    bits: u16,
    float: bool,
    rate: u32,
    time_reference: Option<u64>,
}

fn le16(b: &[u8], o: usize) -> u16 {
    b.get(o..o + 2).map_or(0, |x| u16::from_le_bytes([x[0], x[1]]))
}
fn le32(b: &[u8], o: usize) -> u32 {
    b.get(o..o + 4).map_or(0, |x| u32::from_le_bytes([x[0], x[1], x[2], x[3]]))
}
fn le64(b: &[u8], o: usize) -> u64 {
    b.get(o..o + 8).and_then(|x| x.try_into().ok()).map_or(0, u64::from_le_bytes)
}

impl WavSource {
    /// Open a WAV file held in memory.
    pub fn parse(name: &str, bytes: Arc<[u8]>) -> Result<Self> {
        Self::open_reader(name, Arc::new(MemReader(bytes)))
    }

    /// Open a WAV file from a reader: only the headers and metadata chunks are read.
    pub fn open_reader(name: &str, reader: SharedReader) -> Result<Self> {
        let io = |e: std::io::Error| MediaError::Io(format!("{name}: {e}"));
        let len = reader.len();
        let read = |off: u64, n: u64| -> Result<Vec<u8>> {
            let n = usize::try_from(n.min(len.saturating_sub(off))).map_err(|_| MediaError::Decode(format!("{name}: chunk too large")))?;
            let mut v = vec![0u8; n];
            reader.read_at(off, &mut v).map_err(io)?;
            Ok(v)
        };
        let head = read(0, 12)?;
        if !sniff(&head) {
            return Err(MediaError::Decode(format!("{name}: not a WAV file")));
        }
        // RF64 / BW64: the real sizes are in `ds64` (the 32-bit sizes read 0xFFFFFFFF).
        let wide = &head[0..4] != b"RIFF";
        let mut ds64_data: Option<u64> = None;
        let mut pos: u64 = 12;
        let mut fmt = None;
        let mut data: Option<(u64, u64)> = None;
        let mut bext_tr = None;
        let mut ixml_tr = None;
        for _ in 0..MAX_CHUNKS {
            if pos.saturating_add(8) > len {
                break;
            }
            let hdr = read(pos, 8)?;
            let id: [u8; 4] = [hdr[0], hdr[1], hdr[2], hdr[3]];
            let mut size = u64::from(le32(&hdr, 4));
            let body = pos + 8;
            if &id == b"data" {
                if wide && size == 0xFFFF_FFFF {
                    size = ds64_data.unwrap_or(u64::MAX);
                }
                // A recording cut short (or a streamed header with size 0xFFFFFFFF): the rest of the file.
                if size == 0xFFFF_FFFF || body.saturating_add(size) > len {
                    size = len - body;
                }
                data = Some((body, size));
            } else {
                let small = size.min(MAX_META_CHUNK);
                match &id {
                    b"ds64" if size >= 24 => ds64_data = Some(le64(&read(body, 24)?, 8)),
                    b"fmt " if size >= 16 => {
                        let b = read(body, small.min(64))?;
                        let mut tag = le16(&b, 0);
                        let ch = le16(&b, 2);
                        let rate = le32(&b, 4);
                        let align = le16(&b, 12);
                        let bits = le16(&b, 14);
                        if tag == 0xFFFE && b.len() >= 40 {
                            tag = le16(&b, 24);
                        }
                        fmt = Some((tag, ch, rate, align, bits));
                    }
                    // bext: Description 256, Originator 32, OriginatorReference 32, OriginationDate
                    // 10, OriginationTime 8, then TimeReference (low u32, high u32)
                    b"bext" if size >= 346 => {
                        let b = read(body + 338, 8)?;
                        bext_tr = Some(u64::from(le32(&b, 0)) | u64::from(le32(&b, 4)) << 32);
                    }
                    b"iXML" if size <= MAX_META_CHUNK => ixml_tr = ixml_time_reference(&read(body, size)?),
                    _ => {}
                }
            }
            // Chunks after `data` (LIST, iXML, bext at the end of a recording) are still found.
            pos = body.saturating_add(size).saturating_add(size & 1);
            if fmt.is_some() && data.is_some() && pos >= len {
                break;
            }
        }
        let (tag, ch, rate, align, bits) = fmt.ok_or_else(|| MediaError::Decode(format!("{name}: missing fmt chunk")))?;
        let (off, size) = data.ok_or_else(|| MediaError::Decode(format!("{name}: missing data chunk")))?;
        let float = tag == 3;
        let valid = (tag == 1 && matches!(bits, 8 | 16 | 24 | 32)) || (float && matches!(bits, 32 | 64));
        if !valid || ch == 0 || rate == 0 {
            return Err(MediaError::Unsupported(format!("{name}: WAV format tag {tag}, {bits} bits, {ch} channels, {rate} Hz")));
        }
        let sample_bytes = usize::from(bits / 8);
        let block_align = usize::from(align).max(usize::from(ch) * sample_bytes);
        let frames = size / block_align as u64;
        let time_reference = bext_tr.or(ixml_tr);
        let mut info = MediaInfo {
            name: name.into(),
            kind: MediaKind::AudioOnly,
            duration: Tick::from_units(i64::try_from(frames).unwrap_or(i64::MAX), i64::from(rate)),
            video: None,
            audio_streams: vec![AudioStreamInfo {
                sample_rate: rate,
                channels: u32::from(ch),
                codec: if float { "PCM float".into() } else { "PCM".into() },
                bits_per_sample: Some(u32::from(bits)),
            }],
            container: if time_reference.is_some() { "Broadcast WAV".into() } else { "WAV".into() },
            start_timecode: None,
            file_size: Some(len),
        };
        // start timecode: frames at the media's frame rate (the default rate for audio-only media)
        if let Some(tr) = time_reference {
            let r = info.frame_rate();
            let n = tr as i128 * r.num as i128;
            let d = rate as i128 * r.den as i128;
            if d > 0 {
                info.start_timecode = i64::try_from((n + d / 2) / d).ok();
            }
        }
        Ok(Self { info, reader, data_off: off, frames, block_align, channels: usize::from(ch), bits, float, rate, time_reference })
    }

    /// Broadcast Wave start time: samples since midnight.
    pub fn time_reference(&self) -> Option<u64> {
        self.time_reference
    }

    /// Decode frames `first..first + n` (clamped to the file) into `out[channel]`.
    fn read_frames(&self, first: u64, n: usize, out: &mut [Vec<f32>]) -> Result<()> {
        let n = n.min(usize::try_from(self.frames.saturating_sub(first)).unwrap_or(usize::MAX));
        let per_read = (MAX_READ / self.block_align).max(1);
        let mut buf = Vec::new();
        let mut done = 0usize;
        while done < n {
            let k = (n - done).min(per_read);
            buf.resize(k * self.block_align, 0);
            let at = self.data_off + (first + done as u64) * self.block_align as u64;
            self.reader.read_at(at, &mut buf).map_err(|e| MediaError::Io(format!("{}: {e}", self.info.name)))?;
            for f in 0..k {
                let frame = &buf[f * self.block_align..(f + 1) * self.block_align];
                for (c, ch) in out.iter_mut().enumerate().take(self.channels) {
                    ch.push(self.sample(frame, c));
                }
            }
            done += k;
        }
        Ok(())
    }

    fn sample(&self, frame: &[u8], ch: usize) -> f32 {
        let o = ch * usize::from(self.bits / 8);
        let Some(b) = frame.get(o..o + usize::from(self.bits / 8)) else { return 0.0 };
        match (self.bits, self.float) {
            (8, _) => (b[0] as f32 - 128.0) / 128.0,
            (16, _) => i16::from_le_bytes([b[0], b[1]]) as f32 / 32768.0,
            (24, _) => ((i32::from_le_bytes([0, b[0], b[1], b[2]]) >> 8) as f32) / 8_388_608.0,
            (32, false) => i32::from_le_bytes([b[0], b[1], b[2], b[3]]) as f32 / 2_147_483_648.0,
            (32, true) => f32::from_le_bytes([b[0], b[1], b[2], b[3]]),
            (64, true) => f64::from_le_bytes(b.try_into().unwrap_or([0; 8])) as f32,
            _ => 0.0,
        }
    }
}

impl MediaSource for WavSource {
    fn info(&self) -> &MediaInfo {
        &self.info
    }
    fn video_frame(&self, _req: FrameRequest) -> Result<Arc<VideoFrame>> {
        Err(MediaError::NoStream("video"))
    }
    fn audio(&self, start: i64, frames: usize, sample_rate: u32) -> Result<AudioBuffer> {
        let mut out = AudioBuffer::silence(sample_rate, self.channels, frames);
        if frames == 0 || sample_rate == 0 {
            return Ok(out);
        }
        // The source frames this request touches (one extra for interpolation), read in one go.
        let ratio = self.rate as f64 / sample_rate as f64;
        let src_at = |i: usize| (start + i as i64) as f64 * ratio;
        let first = src_at(0).floor().max(0.0) as u64;
        if first >= self.frames {
            return Ok(out);
        }
        let last = (src_at(frames - 1).floor().max(0.0) as u64).saturating_add(1).min(self.frames - 1);
        let span = usize::try_from(last - first + 1).map_err(|_| MediaError::Decode("audio request too long".into()))?;
        let mut src: Vec<Vec<f32>> = (0..self.channels).map(|_| Vec::with_capacity(span)).collect();
        self.read_frames(first, span, &mut src)?;
        let avail = src.first().map_or(0, Vec::len) as u64;
        // Linear-interpolating resample when rates differ (the audio crate provides sinc resampling).
        for i in 0..frames {
            let pos = src_at(i);
            if pos < 0.0 {
                continue;
            }
            let i0 = pos.floor() as u64;
            if i0 >= self.frames {
                break;
            }
            let Some(k) = i0.checked_sub(first).filter(|k| *k < avail) else { continue };
            let k = k as usize;
            let fr = (pos - i0 as f64) as f32;
            for c in 0..self.channels {
                let a = src[c][k];
                let b = src[c].get(k + 1).copied().filter(|_| i0 + 1 < self.frames).unwrap_or(a);
                out.channels[c][i] = a + (b - a) * fr;
            }
        }
        Ok(out)
    }
}

/// TimeReference from an iXML chunk (`<BWF_TIME_REFERENCE_LOW>` / `<BWF_TIME_REFERENCE_HIGH>`).
fn ixml_time_reference(x: &[u8]) -> Option<u64> {
    let text = String::from_utf8_lossy(x);
    let field = |tag: &str| -> Option<u64> {
        let open = format!("<{tag}>");
        let a = text.find(&open)? + open.len();
        let b = a + text[a..].find('<')?;
        text[a..b].trim().parse().ok()
    };
    let low = field("BWF_TIME_REFERENCE_LOW")?;
    Some(low | field("BWF_TIME_REFERENCE_HIGH").unwrap_or(0) << 32)
}

/// Encode interleaved f32 samples as a 16-bit PCM WAV file.
pub fn write_wav16(samples: &[f32], channels: u16, rate: u32) -> Vec<u8> {
    let data_len = samples.len() * 2;
    let mut v = Vec::with_capacity(44 + data_len);
    v.extend_from_slice(b"RIFF");
    v.extend_from_slice(&((36 + data_len) as u32).to_le_bytes());
    v.extend_from_slice(b"WAVEfmt ");
    v.extend_from_slice(&16u32.to_le_bytes());
    v.extend_from_slice(&1u16.to_le_bytes());
    v.extend_from_slice(&channels.to_le_bytes());
    v.extend_from_slice(&rate.to_le_bytes());
    v.extend_from_slice(&(rate * channels as u32 * 2).to_le_bytes());
    v.extend_from_slice(&(channels * 2).to_le_bytes());
    v.extend_from_slice(&16u16.to_le_bytes());
    v.extend_from_slice(b"data");
    v.extend_from_slice(&(data_len as u32).to_le_bytes());
    for s in samples {
        v.extend_from_slice(&((s.clamp(-1.0, 1.0) * 32767.0).round() as i16).to_le_bytes());
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reader::ByteReader;

    #[test]
    fn roundtrip() {
        let s: Vec<f32> = (0..200).map(|i| ((i as f32) * 0.1).sin() * 0.5).collect();
        let bytes = write_wav16(&s, 2, 48000);
        let src = WavSource::parse("t.wav", bytes.into()).unwrap();
        assert_eq!(src.info().audio().unwrap().channels, 2);
        let a = src.audio(0, 100, 48000).unwrap();
        assert!((a.channels[0][3] - s[6]).abs() < 1e-4);
        assert!((a.channels[1][3] - s[7]).abs() < 1e-4);
        assert_eq!(src.info().duration, Tick::from_units(100, 48000));
        assert_eq!(src.time_reference(), None);
        assert_eq!(src.info().start_timecode, None);
    }

    /// Insert a chunk before `data`.
    fn with_chunk(wav: &[u8], id: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut v = wav[..36].to_vec();
        v.extend_from_slice(id);
        v.extend_from_slice(&(body.len() as u32).to_le_bytes());
        v.extend_from_slice(body);
        if body.len() % 2 == 1 {
            v.push(0);
        }
        v.extend_from_slice(&wav[36..]);
        let riff = (v.len() - 8) as u32;
        v[4..8].copy_from_slice(&riff.to_le_bytes());
        v
    }

    #[test]
    fn bwf_time_reference_from_bext() {
        let wav = write_wav16(&[0.0; 96], 2, 48_000);
        // 01:00:00:00 at 23.976 fps: 86 400 frames of 2002 samples
        let tr: u64 = 86_400 * 2002;
        let mut bext = vec![0u8; 602];
        bext[338..342].copy_from_slice(&(tr as u32).to_le_bytes());
        bext[342..346].copy_from_slice(&((tr >> 32) as u32).to_le_bytes());
        let src = WavSource::parse("bwf.wav", with_chunk(&wav, b"bext", &bext).into()).unwrap();
        assert_eq!(src.time_reference(), Some(tr));
        assert_eq!(src.info().frame_rate(), filmcraft_time::FrameRate::FPS_23_976);
        assert_eq!(src.info().start_timecode, Some(86_400));
        assert_eq!(src.info().container, "Broadcast WAV");
        // the audio is unaffected
        assert_eq!(src.info().duration, Tick::from_units(48, 48_000));
    }

    #[test]
    fn bwf_time_reference_from_ixml_and_large_values() {
        let wav = write_wav16(&[0.0; 4], 1, 48_000);
        let tr: u64 = (1 << 32) + 5;
        let xml = format!(
            "<?xml version=\"1.0\"?><BWFXML><BEXT><BWF_TIME_REFERENCE_LOW>{}</BWF_TIME_REFERENCE_LOW><BWF_TIME_REFERENCE_HIGH>{}</BWF_TIME_REFERENCE_HIGH></BEXT></BWFXML>",
            tr & 0xFFFF_FFFF,
            tr >> 32
        );
        let src = WavSource::parse("ixml.wav", with_chunk(&wav, b"iXML", xml.as_bytes()).into()).unwrap();
        assert_eq!(src.time_reference(), Some(tr));
        // a short bext (no TimeReference) is ignored
        let src = WavSource::parse("short.wav", with_chunk(&wav, b"bext", &[0u8; 100]).into()).unwrap();
        assert_eq!(src.time_reference(), None);
    }

    /// A reader that counts the bytes read.
    struct Counting(MemReader, std::sync::atomic::AtomicU64);
    impl ByteReader for Counting {
        fn len(&self) -> u64 {
            self.0.len()
        }
        fn read_at(&self, offset: u64, buf: &mut [u8]) -> std::io::Result<()> {
            self.1.fetch_add(buf.len() as u64, std::sync::atomic::Ordering::Relaxed);
            self.0.read_at(offset, buf)
        }
    }

    /// #279: opening a long WAV reads its headers, not its samples, and a block of audio reads
    /// only that block.
    #[test]
    fn opening_reads_headers_only_and_audio_reads_the_span() {
        let n = 48_000 * 60; // one minute, mono: 5.76 MB of samples
        let s: Vec<f32> = (0..n).map(|i| ((i % 480) as f32 / 480.0) - 0.5).collect();
        let wav = write_wav16(&s, 1, 48_000);
        let r = Arc::new(Counting(MemReader(wav.into()), Default::default()));
        let src = WavSource::open_reader("long.wav", r.clone()).unwrap();
        let opened = r.1.load(std::sync::atomic::Ordering::Relaxed);
        assert!(opened < 1024, "open read {opened} bytes");
        assert_eq!(src.info().duration, Tick::from_units(n as i64, 48_000));
        let a = src.audio(1_000_000, 1024, 48_000).unwrap();
        let read = r.1.load(std::sync::atomic::Ordering::Relaxed) - opened;
        assert!(read <= 1025 * 2, "audio read {read} bytes");
        for i in [0, 1, 500, 1023] {
            assert!((a.channels[0][i] - s[1_000_000 + i]).abs() < 1e-4);
        }
        // past the end: silence, no read error
        let tail = src.audio(n as i64 - 10, 100, 48_000).unwrap();
        assert!((tail.channels[0][5] - s[n - 5]).abs() < 1e-4);
        assert_eq!(tail.channels[0][50], 0.0);
    }

    /// Resampling reads the right span: 44.1 kHz played at 48 kHz.
    #[test]
    fn resampled_reads_match_whole_file_interpolation() {
        let s: Vec<f32> = (0..44_100).map(|i| (i as f32 * 0.01).sin() * 0.8).collect();
        let src = WavSource::parse("r.wav", write_wav16(&s, 1, 44_100).into()).unwrap();
        let a = src.audio(30_000, 256, 48_000).unwrap();
        for i in [0usize, 7, 255] {
            let pos = (30_000 + i) as f64 * 44_100.0 / 48_000.0;
            let i0 = pos.floor() as usize;
            let fr = (pos - i0 as f64) as f32;
            let q = |x: f32| (x.clamp(-1.0, 1.0) * 32767.0).round() / 32768.0;
            let want = q(s[i0]) + (q(s[i0 + 1]) - q(s[i0])) * fr;
            assert!((a.channels[0][i] - want).abs() < 1e-3, "{i}: {} vs {want}", a.channels[0][i]);
        }
    }

    /// RF64 with its sizes in `ds64`, `nBlockAlign` padding, and a chunk after `data`.
    #[test]
    fn rf64_ds64_block_align_and_trailing_chunks() {
        // 24-bit stereo in 8-byte blocks (padding after each frame), 10 frames
        let frames = 10u64;
        let mut data = Vec::new();
        for f in 0..frames {
            for c in 0..2i32 {
                let v = ((f as i32 * 2 + c) * 100_000) << 8;
                data.extend_from_slice(&v.to_le_bytes()[1..4]);
            }
            data.extend_from_slice(&[0xEE, 0xEE]);
        }
        let mut v = Vec::new();
        v.extend_from_slice(b"RF64");
        v.extend_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
        v.extend_from_slice(b"WAVEds64");
        v.extend_from_slice(&28u32.to_le_bytes());
        v.extend_from_slice(&0u64.to_le_bytes());
        v.extend_from_slice(&(data.len() as u64).to_le_bytes());
        v.extend_from_slice(&frames.to_le_bytes());
        v.extend_from_slice(&0u32.to_le_bytes());
        v.extend_from_slice(b"fmt ");
        v.extend_from_slice(&16u32.to_le_bytes());
        v.extend_from_slice(&1u16.to_le_bytes());
        v.extend_from_slice(&2u16.to_le_bytes());
        v.extend_from_slice(&48_000u32.to_le_bytes());
        v.extend_from_slice(&(48_000u32 * 8).to_le_bytes());
        v.extend_from_slice(&8u16.to_le_bytes());
        v.extend_from_slice(&24u16.to_le_bytes());
        v.extend_from_slice(b"data");
        v.extend_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
        v.extend_from_slice(&data);
        let tr: u64 = 48_000 * 3600;
        let xml = format!("<BWFXML><BWF_TIME_REFERENCE_LOW>{tr}</BWF_TIME_REFERENCE_LOW></BWFXML>");
        v.extend_from_slice(b"iXML");
        v.extend_from_slice(&(xml.len() as u32).to_le_bytes());
        v.extend_from_slice(xml.as_bytes());
        let src = WavSource::parse("rf64.wav", v.into()).unwrap();
        assert_eq!(src.info().duration, Tick::from_units(frames as i64, 48_000));
        assert_eq!(src.time_reference(), Some(tr), "iXML after data is still read");
        let a = src.audio(0, 10, 48_000).unwrap();
        for f in 0..10usize {
            for c in 0..2usize {
                let want = ((f * 2 + c) as f32 * 100_000.0) / 8_388_608.0;
                assert!((a.channels[c][f] - want).abs() < 1e-6, "frame {f} ch {c}");
            }
        }
    }

    /// Truncated, empty and hostile headers are errors, never panics.
    #[test]
    fn hostile_headers_are_errors() {
        let wav = write_wav16(&[0.25; 64], 2, 48_000);
        for cut in [0, 4, 12, 20, 36, 44, 60] {
            let _ = WavSource::parse("cut.wav", wav[..cut.min(wav.len())].to_vec().into());
        }
        let mut zero_rate = wav.clone();
        zero_rate[24..28].copy_from_slice(&0u32.to_le_bytes());
        assert!(WavSource::parse("z.wav", zero_rate.into()).is_err());
        let mut zero_align = wav.clone();
        zero_align[32..34].copy_from_slice(&0u16.to_le_bytes());
        let src = WavSource::parse("a.wav", zero_align.into()).unwrap();
        assert!((src.audio(0, 4, 48_000).unwrap().channels[1][3] - 0.25).abs() < 1e-3);
        let mut huge = wav.clone();
        huge[40..44].copy_from_slice(&0x7FFF_FFF0u32.to_le_bytes());
        let src = WavSource::parse("h.wav", huge.into()).unwrap();
        assert_eq!(src.info().duration, Tick::from_units(64 / 2, 48_000));
        assert!(src.audio(i64::MAX / 4, 16, 48_000).is_ok());
        assert!(src.audio(-1000, 16, 7).is_ok());
    }
}
