use std::fs::File;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::{
    CODEC_TYPE_ALAC, CODEC_TYPE_FLAC, CODEC_TYPE_NULL, CODEC_TYPE_OPUS, CodecParameters, Decoder as SymDecoder, DecoderOptions,
};
use symphonia::core::errors::Error;
use symphonia::core::formats::{FormatOptions, FormatReader, SeekMode, SeekTo, Track};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::{MetadataOptions, MetadataRevision, StandardTagKey, StandardVisualKey};
use symphonia::core::probe::Hint;
use symphonia::core::units::{Time, TimeBase};

use super::downmix::{Downmix, roles_from_channels, vorbis_roles};
use super::{ogg, opus};

/// Lossy codecs carry state from one block to the next, so decoding starts this far before a
/// seek target and the lead-in is thrown away.
const PREROLL: Duration = Duration::from_millis(120);
/// Opus predicts each frame's loudness from the last, and a fresh decoder takes a few hundred
/// milliseconds to settle on a steady tone. Decoding a second of lead-in costs almost nothing.
const OPUS_PREROLL: Duration = Duration::from_millis(1000);
/// A run of undecodable packets this long means the file is not worth playing on.
const MAX_BAD_PACKETS: u32 = 400;

#[derive(Debug)]
pub enum OpenError {
    Io(String),
    /// A format or codec we cannot decode, such as WMA.
    Unsupported(String),
    Failed(String),
}

impl std::fmt::Display for OpenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(text) | Self::Unsupported(text) | Self::Failed(text) => f.write_str(text),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cover {
    pub mime: String,
    pub data: Vec<u8>,
}

#[derive(Clone, Debug, Default)]
pub struct TrackInfo {
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub duration: Option<Duration>,
    pub sample_rate: u32,
    /// Channels in the file. The player mixes more than two down to stereo.
    pub channels: usize,
    pub codec: String,
    pub cover: Option<Arc<Cover>>,
}

enum Codec {
    Symphonia(Box<dyn SymDecoder>),
    Opus(OpusState),
}

struct OpusState {
    decoder: opus::Decoder,
    // Frames the stream really holds, once the pre-skip is gone. Read from the last Ogg page.
    total: Option<u64>,
    // Frames decoded since the start of the stream, pre-skip included.
    raw_pos: u64,
}

pub struct FileSource {
    format: Box<dyn FormatReader>,
    codec: Codec,
    track_id: u32,
    time_base: Option<TimeBase>,
    rate: u32,
    downmix: Downmix,
    info: TrackInfo,
    // Timeline position of the next frame read() will hand out.
    position: u64,
    // Frames still to drop after a seek, to land exactly on the target.
    discard: u64,
    // After a seek: the frame to land on. Packet timestamps tell how much of each block to drop.
    align_to: Option<u64>,
    // The drop to use when a packet carries no timestamps.
    align_fallback: u64,
    raw: Vec<f32>,
    carry: Vec<f32>,
    sample_buf: Option<SampleBuffer<f32>>,
    bad: u32,
    done: bool,
    // The block just decoded was the last one the stream holds.
    last_block: bool,
    lossless: bool,
}

impl FileSource {
    pub fn open(path: &Path) -> Result<Self, OpenError> {
        let mut opened = probe(path, true)?;
        // Symphonia's own gapless handling of Opus is unreliable, so Opus is trimmed here from
        // the raw stream.
        if pick_track(opened.format.tracks()).is_some_and(|track| track.codec_params.codec == CODEC_TYPE_OPUS) {
            let tags = opened.info;
            opened = probe(path, false)?;
            opened.info = tags;
        }
        let Opened { format, mut info } = opened;

        let track = pick_track(format.tracks()).ok_or_else(|| OpenError::Unsupported("The file has no sound track".into()))?;
        let params = track.codec_params.clone();
        let track_id = track.id;
        let time_base = params.time_base;
        info.codec = codec_label(&params);
        info.duration = duration_of(&params, time_base);

        let (codec, roles) = if params.codec == CODEC_TYPE_OPUS {
            let head_bytes = params.extra_data.as_deref().ok_or_else(|| OpenError::Failed("The Opus header is missing".into()))?;
            let head = opus::parse_head(head_bytes).map_err(OpenError::Unsupported)?;
            let roles = if head.vorbis_order {
                vorbis_roles(head.channels).ok_or_else(|| OpenError::Unsupported(format!("{} Opus channels are not supported", head.channels)))?
            } else if head.channels == 1 {
                vec![super::downmix::Role::Left]
            } else {
                vec![super::downmix::Role::Left, super::downmix::Role::Right]
            };
            // The stream is 48 kHz whatever rate the encoder took in.
            let total = ogg::last_granule(path).map(|granule| granule.saturating_sub(head.pre_skip));
            let total = total.or_else(|| params.n_frames.map(|n| n.saturating_sub(head.pre_skip)));
            info.duration = total.filter(|&n| n > 0).map(|n| Duration::from_secs_f64(n as f64 / 48_000.0));
            let decoder = opus::Decoder::new(head).map_err(OpusError::into_open)?;
            (Codec::Opus(OpusState { decoder, total, raw_pos: 0 }), roles)
        } else {
            let decoder = symphonia::default::get_codecs()
                .make(&params, &DecoderOptions::default())
                .map_err(|err| OpenError::Unsupported(format!("{}: {err}", info.codec)))?;
            let roles = params.channels.map(roles_from_channels).unwrap_or_default();
            (Codec::Symphonia(decoder), roles)
        };
        info.sample_rate = if params.codec == CODEC_TYPE_OPUS { 48_000 } else { params.sample_rate.unwrap_or(0) };
        info.channels = roles.len();

        let mut source = Self {
            format,
            codec,
            track_id,
            time_base,
            rate: info.sample_rate,
            downmix: Downmix::new(roles),
            info,
            position: 0,
            discard: 0,
            align_to: None,
            align_fallback: 0,
            raw: Vec::new(),
            carry: Vec::new(),
            sample_buf: None,
            bad: 0,
            done: false,
            last_block: false,
            lossless: is_lossless(&params),
        };
        if source.rate == 0 || source.downmix.inputs() == 0 {
            source.prime()?;
        }
        if source.rate == 0 || source.downmix.inputs() == 0 {
            return Err(OpenError::Failed("The file has no readable audio".into()));
        }
        Ok(source)
    }

    pub fn info(&self) -> &TrackInfo {
        &self.info
    }

    pub fn sample_rate(&self) -> u32 {
        self.rate
    }

    /// Channels that read() delivers: 1 for a mono file, otherwise 2.
    pub fn channels(&self) -> usize {
        self.downmix.outputs()
    }

    pub fn position_frames(&self) -> u64 {
        self.position
    }

    pub fn position(&self) -> Duration {
        Duration::from_secs_f64(self.position as f64 / f64::from(self.rate.max(1)))
    }

    /// Learns the layout from the first decoded packet, for files that do not state it up front.
    fn prime(&mut self) -> Result<(), OpenError> {
        let mut block = Vec::new();
        match self.read(&mut block) {
            Ok(true) => {
                self.carry = block;
                // The frames in carry are the start of the file, so the clock starts before them.
                self.position = 0;
                Ok(())
            }
            Ok(false) => Err(OpenError::Failed("The file has no readable audio".into())),
            Err(err) => Err(OpenError::Failed(err)),
        }
    }

    /// Appends the next block of interleaved frames. Returns false at the end of the file.
    pub fn read(&mut self, out: &mut Vec<f32>) -> Result<bool, String> {
        if !self.carry.is_empty() {
            let frames = self.carry.len() / self.downmix.outputs().max(1);
            out.append(&mut self.carry);
            self.position += frames as u64;
            return Ok(true);
        }
        loop {
            if self.done {
                return Ok(false);
            }
            let packet = match self.format.next_packet() {
                Ok(packet) => packet,
                Err(Error::ResetRequired) => {
                    self.reset_codec()?;
                    continue;
                }
                // The end of the stream arrives as an IO error. So does a read failure, and either way there is no more.
                Err(Error::IoError(_)) => {
                    self.done = true;
                    return Ok(false);
                }
                Err(err) => {
                    self.done = true;
                    return Err(err.to_string());
                }
            };
            if packet.track_id() != self.track_id {
                continue;
            }
            self.raw.clear();
            self.last_block = false;
            match self.decode(&packet) {
                Ok(()) => self.bad = 0,
                Err(Error::ResetRequired) => {
                    self.reset_codec()?;
                    continue;
                }
                Err(Error::DecodeError(_)) | Err(Error::IoError(_)) => {
                    // A damaged packet costs a moment of sound. Many in a row end the track.
                    self.bad += 1;
                    if self.bad > MAX_BAD_PACKETS {
                        self.done = true;
                        return Err("Too much of the file is damaged".into());
                    }
                    continue;
                }
                Err(err) => {
                    self.done = true;
                    return Err(err.to_string());
                }
            }
            if self.last_block {
                self.done = true;
            }
            if self.downmix.inputs() == 0 || self.raw.is_empty() {
                continue;
            }
            let before = out.len();
            self.downmix.apply(&self.raw, out);
            let width = self.downmix.outputs();
            if self.discard > 0 {
                let made = (out.len() - before) / width;
                let drop = (self.discard as usize).min(made);
                out.drain(before..before + drop * width);
                self.discard -= drop as u64;
            }
            let made = (out.len() - before) / width;
            if made == 0 {
                continue;
            }
            self.position += made as u64;
            return Ok(true);
        }
    }

    /// Decodes one packet into `self.raw` in the file's own channel layout.
    fn decode(&mut self, packet: &symphonia::core::formats::Packet) -> Result<(), Error> {
        match &mut self.codec {
            Codec::Opus(state) => {
                let channels = state.decoder.head().channels;
                let pre_skip = state.decoder.head().pre_skip as i64;
                // After a seek the first packet says where the stream really resumed.
                if let Some(target) = self.align_to.take() {
                    state.raw_pos = packet.ts;
                    let first = packet.ts as i64 - pre_skip;
                    self.discard = target.saturating_sub(first.max(0) as u64);
                }
                let frames = state
                    .decoder
                    .decode(packet.buf(), &mut self.raw)
                    .map_err(|_| Error::DecodeError("bad Opus packet"))?;
                // Timeline index of this packet's first frame. The pre-skip comes before zero.
                let first = state.raw_pos as i64 - pre_skip;
                state.raw_pos += frames as u64;
                let front = (-first).clamp(0, frames as i64) as usize;
                let end = match state.total {
                    Some(total) => (total as i64 - first).clamp(0, frames as i64) as usize,
                    None => frames,
                };
                if state.total.is_some_and(|total| first + frames as i64 >= total as i64) {
                    self.last_block = true;
                }
                let end = end.max(front);
                self.raw.truncate(end * channels);
                self.raw.drain(..front * channels);
                Ok(())
            }
            Codec::Symphonia(decoder) => {
                let decoded = decoder.decode(packet)?;
                let spec = *decoded.spec();
                let channels = spec.channels.count();
                if self.downmix.inputs() == 0 {
                    self.downmix = Downmix::new(roles_from_channels(spec.channels));
                    self.rate = spec.rate;
                    self.info.sample_rate = spec.rate;
                    self.info.channels = channels;
                }
                if channels == 0 || channels != self.downmix.inputs() {
                    // The layout changed mid-stream. Skip rather than mix with the wrong gains.
                    return Err(Error::DecodeError("channel layout changed"));
                }
                let need = decoded.capacity() * channels;
                if self.sample_buf.as_ref().is_none_or(|buf| buf.capacity() < need) {
                    self.sample_buf = Some(SampleBuffer::<f32>::new(decoded.capacity() as u64, spec));
                }
                let buf = self.sample_buf.as_mut().expect("sample buffer was just made");
                buf.copy_interleaved_ref(decoded);
                self.raw.extend_from_slice(buf.samples());
                if let Some(target) = self.align_to {
                    self.align_after_seek(target, channels, packet.ts, packet.dur);
                }
                Ok(())
            }
        }
    }

    // A block's frames end where its timestamp plus duration says. The first block after a seek
    // may hold fewer than that (Vorbis yields none), so this finds where the frames really start.
    fn align_after_seek(&mut self, target: u64, channels: usize, ts: u64, dur: u64) {
        if dur == 0 {
            self.align_to = None;
            self.discard = self.align_fallback;
            return;
        }
        let have = (self.raw.len() / channels) as u64;
        let start = self.frames_of(ts + dur).saturating_sub(have);
        if start >= target {
            self.align_to = None;
            if start > target {
                self.position = start;
            }
            return;
        }
        let drop = (target - start).min(have) as usize;
        self.raw.drain(..drop * channels);
        if drop as u64 <= have && (target - start) <= have {
            self.align_to = None;
        }
    }

    fn reset_codec(&mut self) -> Result<(), String> {
        match &mut self.codec {
            Codec::Symphonia(decoder) => decoder.reset(),
            Codec::Opus(state) => {
                let head = state.decoder.head().clone();
                state.decoder = opus::Decoder::new(head)?;
            }
        }
        Ok(())
    }

    /// Moves playback to `to`, landing within a frame of it.
    pub fn seek(&mut self, to: Duration) -> Result<(), String> {
        let to = match self.info.duration {
            Some(total) if to > total => total,
            _ => to,
        };
        let preroll = match &self.codec {
            Codec::Opus(_) => OPUS_PREROLL,
            Codec::Symphonia(_) if self.lossless => Duration::ZERO,
            Codec::Symphonia(_) => PREROLL,
        };
        let want = to.saturating_sub(preroll);
        let rate = f64::from(self.rate);
        let target_frames = (to.as_secs_f64() * rate).round() as u64;
        // Opus counts from the first decoded frame, which sits a pre-skip before the audio.
        let pre_skip = match &self.codec {
            Codec::Opus(state) => state.decoder.head().pre_skip,
            Codec::Symphonia(_) => 0,
        };
        let aim = want.as_secs_f64() + pre_skip as f64 / rate;
        let seek_to = SeekTo::Time { time: Time::from(aim), track_id: Some(self.track_id) };
        self.carry.clear();
        self.done = false;
        self.last_block = false;
        self.align_to = None;
        match self.format.seek(SeekMode::Accurate, seek_to) {
            Ok(seeked) => {
                self.reset_codec()?;
                self.bad = 0;
                if let Codec::Opus(state) = &mut self.codec {
                    // The discard is worked out from the first packet that arrives.
                    state.raw_pos = seeked.actual_ts;
                    self.discard = 0;
                    self.align_to = Some(target_frames);
                    self.position = target_frames;
                    return Ok(());
                }
                let actual = self.frames_of(seeked.actual_ts);
                let required = self.frames_of(seeked.required_ts);
                let lead = target_frames.saturating_sub((want.as_secs_f64() * rate).round() as u64);
                self.discard = 0;
                self.align_to = Some(target_frames);
                self.align_fallback = if actual <= required { (required - actual) + lead } else { 0 };
                self.position = target_frames;
                Ok(())
            }
            // Past the end: park there, and the next read reports the end.
            Err(Error::SeekError(symphonia::core::errors::SeekErrorKind::OutOfRange)) | Err(Error::IoError(_)) => {
                self.discard = 0;
                self.position = target_frames;
                self.done = true;
                Ok(())
            }
            Err(Error::SeekError(_)) => Err("This file cannot be searched".into()),
            Err(err) => Err(err.to_string()),
        }
    }

    fn frames_of(&self, ts: u64) -> u64 {
        match self.time_base {
            Some(base) => {
                let time = base.calc_time(ts);
                ((time.seconds as f64 + time.frac) * f64::from(self.rate)).round() as u64
            }
            None => ts,
        }
    }
}

struct Opened {
    format: Box<dyn FormatReader>,
    info: TrackInfo,
}

fn probe(path: &Path, gapless: bool) -> Result<Opened, OpenError> {
    let file = File::open(path).map_err(|err| OpenError::Io(io_message(path, &err)))?;
    let len = file.metadata().map(|m| m.len()).unwrap_or(0);
    if len == 0 {
        return Err(OpenError::Failed("The file is empty".into()));
    }
    let stream = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|ext| ext.to_str()) {
        hint.with_extension(ext);
    }
    let options = FormatOptions { enable_gapless: gapless, ..Default::default() };
    let mut probed = symphonia::default::get_probe()
        .format(&hint, stream, &options, &MetadataOptions::default())
        .map_err(|err| match err {
            Error::Unsupported(_) => OpenError::Unsupported("Not an audio format the player can read".into()),
            Error::IoError(_) => OpenError::Failed("The file is damaged or cut short".into()),
            other => OpenError::Failed(other.to_string()),
        })?;
    let mut info = TrackInfo::default();
    if let Some(log) = probed.metadata.get()
        && let Some(rev) = log.current()
    {
        absorb_tags(&mut info, rev);
    }
    if let Some(rev) = probed.format.metadata().current() {
        absorb_tags(&mut info, rev);
    }
    Ok(Opened { format: probed.format, info })
}

/// Opus errors are plain strings, and opening reports them as unsupported.
struct OpusError;

impl OpusError {
    fn into_open(message: String) -> OpenError {
        OpenError::Unsupported(message)
    }
}

fn is_lossless(params: &CodecParameters) -> bool {
    if params.codec == CODEC_TYPE_FLAC || params.codec == CODEC_TYPE_ALAC {
        return true;
    }
    let name = symphonia::default::get_codecs().get_codec(params.codec).map(|desc| desc.short_name).unwrap_or("");
    name.starts_with("pcm")
}

/// A film track in an mp4 carries no audio codec, so it is skipped for the sound.
fn playable(params: &CodecParameters) -> bool {
    if params.codec == CODEC_TYPE_NULL {
        return false;
    }
    // Opus is decoded by libopus, which is not in Symphonia's codec list.
    params.codec == CODEC_TYPE_OPUS || symphonia::default::get_codecs().get_codec(params.codec).is_some()
}

fn pick_track(tracks: &[Track]) -> Option<&Track> {
    tracks.iter().find(|track| playable(&track.codec_params))
}

/// True when a track is not playable audio. A cover tag is not a track. Unreadable counts, so Enter stays external.
pub fn has_picture(path: &Path) -> bool {
    let Ok(opened) = probe(path, true) else {
        return true;
    };
    opened.format.tracks().iter().any(|track| !playable(&track.codec_params))
}

fn duration_of(params: &CodecParameters, time_base: Option<TimeBase>) -> Option<Duration> {
    let frames = params.n_frames?;
    if frames == 0 {
        return None;
    }
    let seconds = match (time_base, params.sample_rate) {
        (Some(base), _) => {
            let time = base.calc_time(frames);
            time.seconds as f64 + time.frac
        }
        (None, Some(rate)) if rate > 0 => frames as f64 / f64::from(rate),
        _ => return None,
    };
    (seconds.is_finite() && seconds > 0.0).then(|| Duration::from_secs_f64(seconds))
}

fn codec_label(params: &CodecParameters) -> String {
    if params.codec == CODEC_TYPE_OPUS {
        return "Opus".into();
    }
    let name = symphonia::default::get_codecs().get_codec(params.codec).map(|desc| desc.short_name).unwrap_or("audio");
    if name.starts_with("pcm") {
        "PCM".into()
    } else if name == "mp3" {
        "MP3".into()
    } else if name == "flac" {
        "FLAC".into()
    } else if name == "vorbis" {
        "Vorbis".into()
    } else {
        name.to_uppercase()
    }
}

fn io_message(path: &Path, err: &std::io::Error) -> String {
    let name = path.file_name().and_then(|name| name.to_str()).unwrap_or("file");
    match err.kind() {
        std::io::ErrorKind::NotFound => format!("{name} is gone"),
        std::io::ErrorKind::PermissionDenied => format!("No permission to read {name}"),
        _ => format!("Cannot read {name}: {err}"),
    }
}

fn clean(text: String) -> Option<String> {
    let text = text.replace('\0', " ");
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_string())
}

fn absorb_tags(info: &mut TrackInfo, rev: &MetadataRevision) {
    let mut album_artist = None;
    for tag in rev.tags() {
        let slot = match tag.std_key {
            Some(StandardTagKey::TrackTitle) => &mut info.title,
            Some(StandardTagKey::Artist) => &mut info.artist,
            Some(StandardTagKey::Album) => &mut info.album,
            Some(StandardTagKey::AlbumArtist) => &mut album_artist,
            _ => continue,
        };
        if slot.is_none() {
            *slot = clean(tag.value.to_string());
        }
    }
    if info.artist.is_none() {
        info.artist = album_artist;
    }
    if info.cover.is_none() {
        let visual = rev
            .visuals()
            .iter()
            .find(|visual| visual.usage == Some(StandardVisualKey::FrontCover))
            .or_else(|| rev.visuals().first());
        if let Some(visual) = visual
            && !visual.data.is_empty()
        {
            info.cover = Some(Arc::new(Cover { mime: visual.media_type.clone(), data: visual.data.to_vec() }));
        }
    }
}

#[cfg(test)]
mod tests;
