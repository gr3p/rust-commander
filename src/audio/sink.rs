use std::sync::Arc;

/// Where decoded audio goes. Frames are interleaved f32 in the channel count the sink was opened for.
pub trait Sink: Send {
    /// Blocks until the device has accepted every frame.
    fn write(&mut self, samples: &[f32]) -> Result<(), String>;
    /// Holds the audio already queued. Returns false when the device cannot pause, and the
    /// player then flushes and resumes by seeking.
    fn pause(&mut self, on: bool) -> bool;
    /// Drops audio that was queued but not yet heard.
    fn flush(&mut self);
    /// Frames queued but not yet heard.
    fn delay(&mut self) -> u64;
}

/// Opens a sink for a sample rate and channel count.
pub type SinkFactory = Arc<dyn Fn(u32, usize) -> Result<Box<dyn Sink>, String> + Send + Sync>;

/// The ALSA device, or whatever RUST_COMMANDER_AUDIO_DEVICE names. PipeWire and PulseAudio
/// answer to ALSA's `default`.
pub fn system_sink() -> SinkFactory {
    Arc::new(|rate, channels| {
        let device = std::env::var("RUST_COMMANDER_AUDIO_DEVICE").ok().filter(|name| !name.trim().is_empty());
        let device = device.as_deref().unwrap_or("default");
        super::alsa::AlsaSink::open(device, rate, channels).map(|sink| Box::new(sink) as Box<dyn Sink>)
    })
}
