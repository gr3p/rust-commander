use std::ffi::{CStr, CString, c_char, c_int, c_long, c_uint, c_ulong, c_void};
use std::sync::OnceLock;
use std::time::Duration;

use super::dynlib::Lib;
use super::sink::Sink;

type Pcm = *mut c_void;

const PLAYBACK: c_int = 0;
const FORMAT_S16_LE: c_int = 2;
const FORMAT_FLOAT_LE: c_int = 14;
const ACCESS_RW_INTERLEAVED: c_int = 3;
const EINTR: c_int = 4;
const EAGAIN: c_int = 11;
const EPIPE: c_int = 32;
const ESTRPIPE: c_int = 86;
// Audio ahead of the speakers. Small enough that pause and seek answer fast.
const LATENCY_US: c_uint = 120_000;

/// The few ALSA calls a player needs, looked up at run time. The binary then builds without
/// the ALSA headers and still starts on a machine that has no sound library at all.
struct Api {
    open: unsafe extern "C" fn(*mut Pcm, *const c_char, c_int, c_int) -> c_int,
    set_params: unsafe extern "C" fn(Pcm, c_int, c_int, c_uint, c_uint, c_int, c_uint) -> c_int,
    writei: unsafe extern "C" fn(Pcm, *const c_void, c_ulong) -> c_long,
    recover: unsafe extern "C" fn(Pcm, c_int, c_int) -> c_int,
    prepare: unsafe extern "C" fn(Pcm) -> c_int,
    drop: unsafe extern "C" fn(Pcm) -> c_int,
    close: unsafe extern "C" fn(Pcm) -> c_int,
    delay: unsafe extern "C" fn(Pcm, *mut c_long) -> c_int,
    pause: unsafe extern "C" fn(Pcm, c_int) -> c_int,
    strerror: unsafe extern "C" fn(c_int) -> *const c_char,
}

static API: OnceLock<Result<Api, String>> = OnceLock::new();

fn api() -> Result<&'static Api, String> {
    API.get_or_init(load).as_ref().map_err(Clone::clone)
}

fn load() -> Result<Api, String> {
    // The library stays loaded for the life of the process.
    let lib = Lib::open(&["libasound.so.2", "libasound.so"]).ok_or_else(|| "The ALSA library (libasound.so.2) is not installed".to_string())?;
    Ok(Api {
        open: lib.symbol("snd_pcm_open")?,
        set_params: lib.symbol("snd_pcm_set_params")?,
        writei: lib.symbol("snd_pcm_writei")?,
        recover: lib.symbol("snd_pcm_recover")?,
        prepare: lib.symbol("snd_pcm_prepare")?,
        drop: lib.symbol("snd_pcm_drop")?,
        close: lib.symbol("snd_pcm_close")?,
        delay: lib.symbol("snd_pcm_delay")?,
        pause: lib.symbol("snd_pcm_pause")?,
        strerror: lib.symbol("snd_strerror")?,
    })
}

fn describe(api: &Api, code: c_int) -> String {
    let text = unsafe { (api.strerror)(code) };
    if text.is_null() {
        return format!("ALSA error {code}");
    }
    unsafe { CStr::from_ptr(text) }.to_string_lossy().into_owned()
}

/// Whether the ALSA library can be loaded here.
pub fn available() -> bool {
    api().is_ok()
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Format {
    Float,
    Int16,
}

pub struct AlsaSink {
    api: &'static Api,
    pcm: Pcm,
    channels: usize,
    format: Format,
    paused: bool,
    int16: Vec<i16>,
}

// The device handle is only ever used from the thread that owns the sink.
unsafe impl Send for AlsaSink {}

impl AlsaSink {
    pub fn open(device: &str, rate: u32, channels: usize) -> Result<Self, String> {
        let api = api()?;
        if rate == 0 || channels == 0 {
            return Err("The track has no sample rate".into());
        }
        let name = CString::new(device).map_err(|_| "Bad audio device name".to_string())?;
        for format in [Format::Float, Format::Int16] {
            let mut pcm: Pcm = std::ptr::null_mut();
            let code = unsafe { (api.open)(&mut pcm, name.as_ptr(), PLAYBACK, 0) };
            if code < 0 {
                return Err(format!("Cannot open audio device {device}: {}", describe(api, code)));
            }
            let kind = if format == Format::Float { FORMAT_FLOAT_LE } else { FORMAT_S16_LE };
            let code = unsafe { (api.set_params)(pcm, kind, ACCESS_RW_INTERLEAVED, channels as c_uint, rate, 1, LATENCY_US) };
            if code >= 0 {
                return Ok(Self { api, pcm, channels, format, paused: false, int16: Vec::new() });
            }
            unsafe { (api.close)(pcm) };
            if format == Format::Int16 {
                return Err(format!("Audio device {device} refused {rate} Hz: {}", describe(api, code)));
            }
        }
        Err("No usable sample format".into())
    }

    fn write_frames(&mut self, ptr: *const c_void, frames: usize, offset_frames: usize, bytes_per_frame: usize) -> Result<usize, String> {
        let at = unsafe { ptr.cast::<u8>().add(offset_frames * bytes_per_frame) };
        let left = frames - offset_frames;
        let wrote = unsafe { (self.api.writei)(self.pcm, at.cast(), left as c_ulong) };
        if wrote >= 0 {
            return Ok(wrote as usize);
        }
        let code = wrote as c_int;
        match -code {
            EINTR => Ok(0),
            EAGAIN => {
                std::thread::sleep(Duration::from_millis(2));
                Ok(0)
            }
            // An underrun or a suspended machine. Recovering restarts the stream.
            EPIPE | ESTRPIPE => {
                let again = unsafe { (self.api.recover)(self.pcm, code, 1) };
                if again < 0 {
                    Err(describe(self.api, again))
                } else {
                    Ok(0)
                }
            }
            _ => Err(describe(self.api, code)),
        }
    }
}

impl Sink for AlsaSink {
    fn write(&mut self, samples: &[f32]) -> Result<(), String> {
        let frames = samples.len() / self.channels;
        let (ptr, bytes): (*const c_void, usize) = match self.format {
            Format::Float => (samples.as_ptr().cast(), 4 * self.channels),
            Format::Int16 => {
                self.int16.clear();
                self.int16.extend(samples[..frames * self.channels].iter().map(|s| (s.clamp(-1.0, 1.0) * 32767.0) as i16));
                (self.int16.as_ptr().cast(), 2 * self.channels)
            }
        };
        let mut done = 0;
        let mut idle = 0;
        while done < frames {
            let wrote = self.write_frames(ptr, frames, done, bytes)?;
            if wrote == 0 {
                idle += 1;
                if idle > 200 {
                    return Err("The audio device stopped accepting sound".into());
                }
            } else {
                idle = 0;
                done += wrote;
            }
        }
        Ok(())
    }

    fn pause(&mut self, on: bool) -> bool {
        if on == self.paused {
            return true;
        }
        let code = unsafe { (self.api.pause)(self.pcm, on as c_int) };
        if code == 0 {
            self.paused = on;
            true
        } else {
            // Not every device can pause. The caller then flushes and resumes by seeking.
            self.paused = false;
            !on
        }
    }

    fn flush(&mut self) {
        unsafe {
            (self.api.drop)(self.pcm);
            (self.api.prepare)(self.pcm);
        }
        self.paused = false;
    }

    fn delay(&mut self) -> u64 {
        let mut frames: c_long = 0;
        let code = unsafe { (self.api.delay)(self.pcm, &mut frames) };
        if code < 0 { 0 } else { frames.max(0) as u64 }
    }
}

impl Drop for AlsaSink {
    fn drop(&mut self) {
        unsafe {
            (self.api.drop)(self.pcm);
            (self.api.close)(self.pcm);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ALSA's own "null" device accepts and discards audio, so this runs without a sound card.
    fn null_sink(rate: u32, channels: usize) -> Option<AlsaSink> {
        if !available() {
            eprintln!("skipped: no ALSA library");
            return None;
        }
        match AlsaSink::open("null", rate, channels) {
            Ok(sink) => Some(sink),
            Err(err) => {
                eprintln!("skipped: {err}");
                None
            }
        }
    }

    #[test]
    fn writes_float_audio_to_the_null_device() {
        let Some(mut sink) = null_sink(48_000, 2) else { return };
        let tone: Vec<f32> = (0..9_600).flat_map(|i| {
            let s = (i as f32 * 0.05).sin() * 0.2;
            [s, s]
        }).collect();
        sink.write(&tone).unwrap();
        sink.flush();
        sink.write(&tone).unwrap();
    }

    #[test]
    fn mono_and_odd_rates_are_accepted() {
        for (rate, channels) in [(8_000, 1), (44_100, 1), (22_050, 2), (96_000, 2)] {
            let Some(mut sink) = null_sink(rate, channels) else { return };
            sink.write(&vec![0.1; 1_000 * channels]).unwrap();
        }
    }

    #[test]
    fn a_partial_frame_is_ignored_and_empty_writes_are_fine() {
        let Some(mut sink) = null_sink(48_000, 2) else { return };
        sink.write(&[]).unwrap();
        sink.write(&[0.0, 0.0, 0.0]).unwrap();
    }

    #[test]
    fn pause_flush_and_delay_do_not_fail_on_a_fresh_device() {
        let Some(mut sink) = null_sink(48_000, 2) else { return };
        sink.write(&vec![0.0; 4_800]).unwrap();
        let _ = sink.pause(true);
        let _ = sink.pause(false);
        sink.flush();
        let _ = sink.delay();
        sink.write(&vec![0.0; 4_800]).unwrap();
    }

    #[test]
    fn a_missing_device_is_an_error_not_a_panic() {
        if !available() {
            return;
        }
        let err = AlsaSink::open("no-such-device-xyz", 48_000, 2).err().expect("must fail");
        assert!(err.contains("no-such-device-xyz"), "{err}");
    }
}
