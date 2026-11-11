use std::ffi::{CStr, c_char, c_int, c_void};
use std::sync::OnceLock;

use super::dynlib::Lib;

/// libopus, looked up at run time like the ALSA library. Symphonia has no Opus decoder, and this
/// keeps libopus and cmake out of the build.
struct Api {
    create: unsafe extern "C" fn(i32, c_int, c_int, c_int, *const u8, *mut c_int) -> *mut c_void,
    decode: unsafe extern "C" fn(*mut c_void, *const u8, i32, *mut f32, c_int, c_int) -> c_int,
    destroy: unsafe extern "C" fn(*mut c_void),
    strerror: unsafe extern "C" fn(c_int) -> *const c_char,
}

static API: OnceLock<Result<Api, String>> = OnceLock::new();

fn api() -> Result<&'static Api, String> {
    API.get_or_init(load).as_ref().map_err(Clone::clone)
}

fn load() -> Result<Api, String> {
    let lib = Lib::open(&["libopus.so.0", "libopus.so"])
        .ok_or_else(|| "Opus files need the libopus library (libopus0), which is not installed".to_string())?;
    Ok(Api {
        create: lib.symbol("opus_multistream_decoder_create")?,
        decode: lib.symbol("opus_multistream_decode_float")?,
        destroy: lib.symbol("opus_multistream_decoder_destroy")?,
        strerror: lib.symbol("opus_strerror")?,
    })
}

fn describe(api: &Api, code: c_int) -> String {
    let text = unsafe { (api.strerror)(code) };
    if text.is_null() {
        return format!("Opus error {code}");
    }
    unsafe { CStr::from_ptr(text) }.to_string_lossy().into_owned()
}

pub fn available() -> bool {
    api().is_ok()
}

/// The longest Opus frame, 120 ms at 48 kHz.
pub const MAX_FRAME: usize = 5760;

/// What the OpusHead packet says about the stream.
#[derive(Clone, Debug, PartialEq)]
pub struct Head {
    pub channels: usize,
    /// Samples at 48 kHz the decoder produces before the real audio starts.
    pub pre_skip: u64,
    pub gain_db: f32,
    pub streams: usize,
    pub coupled: usize,
    pub mapping: Vec<u8>,
    /// Mapping family 1 lists channels in Vorbis order, so a surround layout is known.
    pub vorbis_order: bool,
}

pub fn parse_head(data: &[u8]) -> Result<Head, String> {
    if data.len() < 19 || &data[..8] != b"OpusHead" {
        return Err("Not an Opus stream".into());
    }
    let channels = data[9] as usize;
    if channels == 0 {
        return Err("Opus stream with no channels".into());
    }
    let pre_skip = u64::from(u16::from_le_bytes([data[10], data[11]]));
    let gain_q8 = i16::from_le_bytes([data[16], data[17]]);
    let gain_db = gain_q8 as f32 / 256.0;
    match data[18] {
        0 => {
            if channels > 2 {
                return Err("Bad Opus channel mapping".into());
            }
            Ok(Head {
                channels,
                pre_skip,
                gain_db,
                streams: 1,
                coupled: usize::from(channels == 2),
                mapping: if channels == 2 { vec![0, 1] } else { vec![0] },
                vorbis_order: false,
            })
        }
        1 => {
            if data.len() < 21 + channels {
                return Err("Short Opus header".into());
            }
            let streams = data[19] as usize;
            let coupled = data[20] as usize;
            // RFC 7845: every stream is mono or a coupled pair, so the channels are streams + coupled.
            if streams == 0 || coupled > streams || streams + coupled != channels {
                return Err("Bad Opus channel mapping".into());
            }
            Ok(Head { channels, pre_skip, gain_db, streams, coupled, mapping: data[21..21 + channels].to_vec(), vorbis_order: true })
        }
        family => Err(format!("Opus channel mapping {family} is not supported")),
    }
}

pub struct Decoder {
    api: &'static Api,
    ptr: *mut c_void,
    head: Head,
    gain: f32,
}

// The decoder is only ever used from the thread that owns it.
unsafe impl Send for Decoder {}

impl Decoder {
    pub fn new(head: Head) -> Result<Self, String> {
        let api = api()?;
        let mut error: c_int = 0;
        let ptr = unsafe {
            (api.create)(48_000, head.channels as c_int, head.streams as c_int, head.coupled as c_int, head.mapping.as_ptr(), &mut error)
        };
        if ptr.is_null() || error != 0 {
            return Err(describe(api, error));
        }
        let gain = 10f32.powf(head.gain_db / 20.0);
        Ok(Self { api, ptr, head, gain })
    }

    pub fn head(&self) -> &Head {
        &self.head
    }

    /// Decodes one packet into interleaved f32 and returns the frames per channel.
    pub fn decode(&mut self, packet: &[u8], out: &mut Vec<f32>) -> Result<usize, String> {
        let start = out.len();
        out.resize(start + MAX_FRAME * self.head.channels, 0.0);
        let frames = unsafe {
            (self.api.decode)(self.ptr, packet.as_ptr(), packet.len() as i32, out[start..].as_mut_ptr(), MAX_FRAME as c_int, 0)
        };
        if frames < 0 {
            out.truncate(start);
            return Err(describe(self.api, frames));
        }
        out.truncate(start + frames as usize * self.head.channels);
        if (self.gain - 1.0).abs() > f32::EPSILON {
            for sample in &mut out[start..] {
                *sample *= self.gain;
            }
        }
        Ok(frames as usize)
    }
}

impl Drop for Decoder {
    fn drop(&mut self) {
        unsafe { (self.api.destroy)(self.ptr) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn head_bytes(channels: u8, family: u8, extra: &[u8]) -> Vec<u8> {
        let mut data = b"OpusHead".to_vec();
        data.extend_from_slice(&[1, channels, 0x38, 0x01, 0x80, 0xbb, 0, 0, 0, 0, family]);
        data.extend_from_slice(extra);
        data
    }

    #[test]
    fn parses_mono_and_stereo_heads() {
        let mono = parse_head(&head_bytes(1, 0, &[])).unwrap();
        assert_eq!((mono.channels, mono.streams, mono.coupled), (1, 1, 0));
        assert_eq!(mono.pre_skip, 312);
        let stereo = parse_head(&head_bytes(2, 0, &[])).unwrap();
        assert_eq!((stereo.channels, stereo.streams, stereo.coupled), (2, 1, 1));
        assert_eq!(stereo.mapping, vec![0, 1]);
        assert!(!stereo.vorbis_order);
    }

    #[test]
    fn parses_a_surround_head_and_the_output_gain() {
        let mut data = head_bytes(6, 1, &[4, 2, 0, 4, 1, 2, 3, 5]);
        // +1 dB output gain in Q7.8.
        data[16..18].copy_from_slice(&256i16.to_le_bytes());
        let head = parse_head(&data).unwrap();
        assert_eq!((head.channels, head.streams, head.coupled), (6, 4, 2));
        assert_eq!(head.mapping, vec![0, 4, 1, 2, 3, 5]);
        assert!(head.vorbis_order);
        assert!((head.gain_db - 1.0).abs() < 1e-6);
    }

    #[test]
    fn rejects_bad_heads() {
        assert!(parse_head(b"short").is_err());
        assert!(parse_head(&head_bytes(0, 0, &[])).is_err());
        assert!(parse_head(&head_bytes(3, 0, &[])).is_err());
        assert!(parse_head(&head_bytes(2, 255, &[])).is_err());
        assert!(parse_head(&head_bytes(6, 1, &[4, 2])).is_err());
        let mut wrong = head_bytes(2, 0, &[]);
        wrong[0] = b'X';
        assert!(parse_head(&wrong).is_err());
    }

    #[test]
    fn a_garbage_packet_is_an_error_not_a_crash() {
        if !available() {
            eprintln!("skipped: no libopus");
            return;
        }
        let mut decoder = Decoder::new(parse_head(&head_bytes(2, 0, &[])).unwrap()).unwrap();
        let mut out = Vec::new();
        // A lone TOC byte for a 20 ms frame is a valid packet; random bytes after it may not be.
        let result = decoder.decode(&[0xff, 0xff, 0xff], &mut out);
        assert!(result.is_err() || out.len() % 2 == 0);
        assert!(out.len() <= MAX_FRAME * 2);
    }
}
