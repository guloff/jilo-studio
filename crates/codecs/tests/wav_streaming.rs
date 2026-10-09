//! #279: importing a WAV goes through a reader opener, so only its head and headers are read,
//! never the whole file.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use filmcraft_media::reader::{ByteReader, MemReader, open_reader};

struct Counting(MemReader, AtomicU64);

impl ByteReader for Counting {
    fn len(&self) -> u64 {
        self.0.len()
    }
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> std::io::Result<()> {
        self.1.fetch_add(buf.len() as u64, Ordering::Relaxed);
        self.0.read_at(offset, buf)
    }
}

#[test]
fn a_long_wav_opens_without_being_read_whole() {
    // 30 s of 48 kHz stereo 16-bit: 5.76 MB
    let samples = vec![0.1f32; 48_000 * 2 * 30];
    let wav = filmcraft_media::wav::write_wav16(&samples, 2, 48_000);
    let size = wav.len() as u64;
    let r = Arc::new(Counting(MemReader(wav.into()), AtomicU64::new(0)));
    let src = open_reader("take.wav", r.clone(), &filmcraft_codecs::reader_openers(), &filmcraft_codecs::openers()).unwrap();
    let read = r.1.load(Ordering::Relaxed);
    assert!(read < 100_000, "opening read {read} of {size} bytes");
    assert_eq!(src.info().audio().unwrap().channels, 2);
    let a = src.audio(48_000 * 20, 480, 48_000).unwrap();
    assert!((a.channels[1][100] - 0.1).abs() < 1e-3);
}
