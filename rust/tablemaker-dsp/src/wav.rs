//! WAV reading (any common PCM or float layout, mixed to mono) and writing,
//! including wavetables tagged with the `clm ` chunk that Vital, Surge XT,
//! Bitwig and others read the frame size from, and Surge XT's own `.wt`.

use std::fs;
use std::io;
use std::path::Path;

/// A mono clip, samples in -1..1.
#[derive(Clone, Debug)]
pub struct Audio {
    pub sample_rate: u32,
    pub samples: Vec<f64>,
}

const FORMAT_PCM: u16 = 1;
const FORMAT_FLOAT: u16 = 3;
const FORMAT_EXTENSIBLE: u16 = 0xFFFE;

fn bad(msg: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg.into())
}

/// Read a WAV file and mix all channels down to mono.
pub fn read(path: &Path) -> io::Result<Audio> {
    parse(&fs::read(path)?)
}

/// Parse an in-memory WAV file and mix all channels down to mono.
pub fn parse(bytes: &[u8]) -> io::Result<Audio> {
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err(bad("not a RIFF/WAVE file"));
    }
    let mut fmt: Option<(u16, u16, u32, u16)> = None; // format, channels, rate, bits
    let mut data: Option<&[u8]> = None;
    let mut pos = 12;
    while pos + 8 <= bytes.len() {
        let id = &bytes[pos..pos + 4];
        let size = u32::from_le_bytes(bytes[pos + 4..pos + 8].try_into().unwrap()) as usize;
        let body_start = pos + 8;
        // A truncated last chunk (common for data written by crashed recorders) is read as far as it goes.
        let body_end = (body_start + size).min(bytes.len());
        let body = &bytes[body_start..body_end];
        match id {
            b"fmt " => {
                if body.len() < 16 {
                    return Err(bad("fmt chunk too short"));
                }
                let mut format = u16::from_le_bytes([body[0], body[1]]);
                let channels = u16::from_le_bytes([body[2], body[3]]);
                let rate = u32::from_le_bytes(body[4..8].try_into().unwrap());
                let bits = u16::from_le_bytes([body[14], body[15]]);
                if format == FORMAT_EXTENSIBLE {
                    if body.len() < 26 {
                        return Err(bad("extensible fmt chunk too short"));
                    }
                    // The sub-format GUID starts with the plain format code.
                    format = u16::from_le_bytes([body[24], body[25]]);
                }
                fmt = Some((format, channels, rate, bits));
            }
            b"data" => data = Some(body),
            _ => {}
        }
        pos = body_start + size + (size & 1);
    }
    let (format, channels, rate, bits) = fmt.ok_or_else(|| bad("no fmt chunk"))?;
    let data = data.ok_or_else(|| bad("no data chunk"))?;
    if channels == 0 || rate == 0 {
        return Err(bad("zero channels or sample rate"));
    }
    let width = (bits as usize).div_ceil(8);
    let decode: fn(&[u8]) -> f64 = match (format, bits) {
        (FORMAT_PCM, 8) => |b| (b[0] as f64 - 128.0) / 128.0,
        (FORMAT_PCM, 16) => |b| i16::from_le_bytes([b[0], b[1]]) as f64 / 32768.0,
        (FORMAT_PCM, 24) => |b| (i32::from_le_bytes([0, b[0], b[1], b[2]]) >> 8) as f64 / 8388608.0,
        (FORMAT_PCM, 32) => |b| i32::from_le_bytes(b.try_into().unwrap()) as f64 / 2147483648.0,
        (FORMAT_FLOAT, 32) => |b| f32::from_le_bytes(b.try_into().unwrap()) as f64,
        (FORMAT_FLOAT, 64) => |b| f64::from_le_bytes(b.try_into().unwrap()),
        _ => {
            return Err(bad(format!(
                "unsupported WAV format {format} at {bits} bits"
            )))
        }
    };
    let channels = channels as usize;
    let frame = width * channels;
    let samples = data
        .chunks_exact(frame)
        .map(|f| f.chunks_exact(width).map(decode).sum::<f64>() / channels as f64)
        .collect();
    Ok(Audio {
        sample_rate: rate,
        samples,
    })
}

/// Sample encoding for written files.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Encoding {
    Float32,
    Pcm16,
    Pcm24,
}

fn encode(samples: &[f64], encoding: Encoding) -> (u16, u16, Vec<u8>) {
    let mut out = Vec::with_capacity(samples.len() * 4);
    match encoding {
        Encoding::Float32 => {
            for &s in samples {
                out.extend_from_slice(&(s as f32).to_le_bytes());
            }
            (FORMAT_FLOAT, 32, out)
        }
        Encoding::Pcm16 => {
            for &s in samples {
                let v = (s.clamp(-1.0, 1.0) * 32767.0).round() as i16;
                out.extend_from_slice(&v.to_le_bytes());
            }
            (FORMAT_PCM, 16, out)
        }
        Encoding::Pcm24 => {
            for &s in samples {
                let v = (s.clamp(-1.0, 1.0) * 8388607.0).round() as i32;
                out.extend_from_slice(&v.to_le_bytes()[..3]);
            }
            (FORMAT_PCM, 24, out)
        }
    }
}

fn chunk(out: &mut Vec<u8>, id: &[u8; 4], body: &[u8]) {
    out.extend_from_slice(id);
    out.extend_from_slice(&(body.len() as u32).to_le_bytes());
    out.extend_from_slice(body);
    if body.len() & 1 == 1 {
        out.push(0);
    }
}

/// Build a mono WAV file in memory, optionally with a `clm ` chunk.
pub fn build(
    samples: &[f64],
    sample_rate: u32,
    encoding: Encoding,
    clm_frame: Option<usize>,
) -> Vec<u8> {
    let (format, bits, data) = encode(samples, encoding);
    let block = bits / 8;
    let mut fmt = Vec::new();
    fmt.extend_from_slice(&format.to_le_bytes());
    fmt.extend_from_slice(&1u16.to_le_bytes());
    fmt.extend_from_slice(&sample_rate.to_le_bytes());
    fmt.extend_from_slice(&(sample_rate * block as u32).to_le_bytes());
    fmt.extend_from_slice(&block.to_le_bytes());
    fmt.extend_from_slice(&bits.to_le_bytes());
    if format == FORMAT_FLOAT {
        fmt.extend_from_slice(&0u16.to_le_bytes()); // cbSize
    }
    let mut body = b"WAVE".to_vec();
    chunk(&mut body, b"fmt ", &fmt);
    if format == FORMAT_FLOAT {
        chunk(&mut body, b"fact", &(samples.len() as u32).to_le_bytes());
    }
    if let Some(frame) = clm_frame {
        // "<!>" then the frame size padded to four characters: the de facto layout
        // every reader of this chunk expects.
        let text = format!("<!>{frame:<4} 00000000 wavetable (TableMaker)");
        chunk(&mut body, b"clm ", text.as_bytes());
    }
    chunk(&mut body, b"data", &data);
    let mut out = b"RIFF".to_vec();
    out.extend_from_slice(&(body.len() as u32).to_le_bytes());
    out.extend_from_slice(&body);
    out
}

/// Write a mono WAV file.
pub fn write(path: &Path, samples: &[f64], sample_rate: u32, encoding: Encoding) -> io::Result<()> {
    fs::write(path, build(samples, sample_rate, encoding, None))
}

/// Write a wavetable: the frames back to back, tagged with their size.
pub fn write_wavetable(
    path: &Path,
    frames: &[Vec<f64>],
    sample_rate: u32,
    encoding: Encoding,
) -> io::Result<()> {
    let size = frames.first().map_or(0, Vec::len);
    let flat: Vec<f64> = frames.iter().flatten().copied().collect();
    fs::write(path, build(&flat, sample_rate, encoding, Some(size)))
}

/// Build a Surge XT `.wt` wavetable (see Surge's "WT fileformat.txt"):
/// "vawt", frame size (power of two, 2..=4096), frame count (1..=512),
/// flags, then the frames. 16-bit data uses the full range (flag 0x8).
pub fn build_wt(frames: &[Vec<f64>], encoding: Encoding) -> Result<Vec<u8>, String> {
    let size = frames.first().map_or(0, Vec::len);
    if !(2..=4096).contains(&size) || !size.is_power_of_two() {
        return Err(format!(
            ".wt frames must be a power of two from 2 to 4096 samples, not {size}"
        ));
    }
    if frames.is_empty() || frames.len() > 512 {
        return Err(format!(".wt holds 1 to 512 frames, not {}", frames.len()));
    }
    let flags: u16 = match encoding {
        Encoding::Float32 => 0,
        Encoding::Pcm16 => 0x4 | 0x8,
        Encoding::Pcm24 => return Err(".wt has no 24-bit format: use float32 or pcm16".into()),
    };
    let mut out = b"vawt".to_vec();
    out.extend_from_slice(&(size as u32).to_le_bytes());
    out.extend_from_slice(&(frames.len() as u16).to_le_bytes());
    out.extend_from_slice(&flags.to_le_bytes());
    let flat: Vec<f64> = frames.iter().flatten().copied().collect();
    out.extend_from_slice(&encode(&flat, encoding).2);
    Ok(out)
}

/// The frame size from a `clm ` chunk, if the file has one.
pub fn clm_frame_size(bytes: &[u8]) -> Option<usize> {
    let at = bytes.windows(4).position(|w| w == b"clm ")?;
    let body = bytes.get(at + 8..)?;
    let text = body.strip_prefix(b"<!>")?;
    let digits: String = text
        .iter()
        .take_while(|b| b.is_ascii_digit())
        .map(|&b| b as char)
        .collect();
    digits.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_every_encoding() {
        let s: Vec<f64> = (0..100).map(|i| ((i as f64) * 0.1).sin() * 0.9).collect();
        for (enc, tol) in [
            (Encoding::Float32, 1e-6),
            (Encoding::Pcm16, 1e-4),
            (Encoding::Pcm24, 1e-6),
        ] {
            let a = parse(&build(&s, 44100, enc, None)).unwrap();
            assert_eq!(a.sample_rate, 44100);
            assert_eq!(a.samples.len(), s.len());
            for (x, y) in a.samples.iter().zip(&s) {
                assert!((x - y).abs() < tol, "{enc:?}: {x} vs {y}");
            }
        }
    }

    #[test]
    fn writes_the_clm_chunk() {
        let bytes = build(&vec![0.0; 2048 * 4], 44100, Encoding::Float32, Some(2048));
        assert_eq!(clm_frame_size(&bytes), Some(2048));
        assert_eq!(parse(&bytes).unwrap().samples.len(), 2048 * 4);
        let small = build(&vec![0.0; 256], 44100, Encoding::Float32, Some(256));
        assert_eq!(clm_frame_size(&small), Some(256));
    }

    #[test]
    fn writes_surge_wt() {
        let frames = vec![vec![0.5; 256], vec![-0.25; 256]];
        let b = build_wt(&frames, Encoding::Float32).unwrap();
        assert_eq!(&b[0..4], b"vawt");
        assert_eq!(u32::from_le_bytes(b[4..8].try_into().unwrap()), 256);
        assert_eq!(u16::from_le_bytes([b[8], b[9]]), 2);
        assert_eq!(u16::from_le_bytes([b[10], b[11]]), 0);
        assert_eq!(b.len(), 12 + 4 * 512);
        assert_eq!(
            f32::from_le_bytes(b[12 + 4 * 256..16 + 4 * 256].try_into().unwrap()),
            -0.25
        );
        let i16s = build_wt(&frames, Encoding::Pcm16).unwrap();
        assert_eq!(u16::from_le_bytes([i16s[10], i16s[11]]), 0xC);
        assert_eq!(i16::from_le_bytes([i16s[12], i16s[13]]), 16384);
        assert!(build_wt(&[vec![0.0; 300]], Encoding::Float32).is_err());
        assert!(build_wt(&frames, Encoding::Pcm24).is_err());
    }

    #[test]
    fn mixes_stereo_to_mono() {
        // 16-bit stereo, one frame: left = 0.5, right = -0.25.
        let mut fmt = Vec::new();
        for v in [1u16, 2] {
            fmt.extend_from_slice(&v.to_le_bytes());
        }
        fmt.extend_from_slice(&48000u32.to_le_bytes());
        fmt.extend_from_slice(&(48000u32 * 4).to_le_bytes());
        fmt.extend_from_slice(&4u16.to_le_bytes());
        fmt.extend_from_slice(&16u16.to_le_bytes());
        let mut data = Vec::new();
        data.extend_from_slice(&16384i16.to_le_bytes());
        data.extend_from_slice(&(-8192i16).to_le_bytes());
        let mut body = b"WAVE".to_vec();
        chunk(&mut body, b"fmt ", &fmt);
        chunk(&mut body, b"data", &data);
        let mut file = b"RIFF".to_vec();
        file.extend_from_slice(&(body.len() as u32).to_le_bytes());
        file.extend_from_slice(&body);
        let a = parse(&file).unwrap();
        assert_eq!(a.sample_rate, 48000);
        assert!((a.samples[0] - 0.125).abs() < 1e-9);
    }
}
