//! Nintendo Switch (Horizon) backend for Bevy 0.18's bevy_audio (through rodio 0.20.1), on cpal
//! 0.15.3's API: one output device, which a thread of the stream's own feeds to libnx's audio
//! renderer (audren). build.rs links libnx. Bevy 0.19's host (patches/cpal) is the same code on
//! cpal 0.17.3's API.
//!
//! The game writes f32; the thread converts it to the 16-bit PCM the renderer's voice takes. It
//! keeps three buffers queued and, every renderer frame (5 ms), refills whichever the renderer has
//! finished, so a thread that runs late costs a gap in the sound, never the sound.
//!
//! Both hosts used to go through devkitPro's SDL2 2.28.5. SDL's Switch backend queues a buffer
//! and then waits for the renderer
//! to report that buffer playing. When the renderer starts and finishes it between two of the
//! thread's updates, a stall of one 21 ms buffer, libnx reports it done instead, the wait never
//! ends, and the game is silent from then on (docs/input-and-audio.md).
//!
//! The thread runs at priority 0x2B, ahead of the game's own threads at 0x3B, which on a busy core
//! take turns of about 10 ms. Three variables, read when the stream is built:
//! - `BH_AUDIO_PRIORITY`: the audio thread's priority, decimal or `0x` hex; `default` keeps the
//!   one it starts with.
//! - `BH_AUDIO_FRAMES`: each buffer, in frames, 256 to 8192, whatever the stream asks for. Unset,
//!   it is what the stream asks for, or 1024 (21 ms at 48 kHz) for the default, as rodio 0.20 does.
//! - `BH_AUDIO_STATS=1`: an `[Audio]` line on stderr every 10 s: how long the callbacks took, the
//!   longest time between two of them, and how often the renderer ran out of sound.
//!
//! It also closes, at exit, a stream that was never dropped: Bevy 0.18's bevy_audio leaks its
//! stream (`mem::forget` in `AudioOutput::default`). audren holds transfer memory, taken from the
//! heap, until it is closed; locked heap pages make bevy_horizon's exit hook end the process
//! through AM, to HOME, instead of returning to the homebrew loader (its
//! docs/lifecycle-and-exit.md). newlib's `exit` runs `atexit` handlers before that hook, so
//! `close_at_exit` closes such streams there.

use std::alloc::Layout;
use std::ffi::{c_char, c_int, c_void};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, Once, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::traits::{DeviceTrait, HostTrait, StreamTrait};
use crate::{
    BackendSpecificError, BuildStreamError, BufferSize, Data, DefaultStreamConfigError,
    DeviceNameError, DevicesError, InputCallbackInfo, OutputCallbackInfo, OutputStreamTimestamp,
    PauseStreamError, PlayStreamError, SampleFormat, SampleRate, StreamConfig, StreamError,
    StreamInstant, SupportedBufferSize, SupportedStreamConfig, SupportedStreamConfigRange,
    SupportedStreamConfigsError,
};

const SAMPLE_RATE: u32 = 48_000;
const MIN_FRAMES: u32 = 256;
const MAX_FRAMES: u32 = 8192;
const DEFAULT_FRAMES: u32 = 1024;
/// Buffers queued on the voice: the renderer plays two while the thread refills the third.
const BUFFERS: usize = 3;
/// The longest the thread waits for the renderer's next frame before it looks at the buffers.
const FRAME_WAIT_NS: u64 = 10_000_000;
/// What SDL gives a high-priority thread (`SDL_THREAD_PRIORITY_HIGH`).
const DEFAULT_PRIORITY: u32 = 0x2B;
/// The kernel's handle for the calling thread.
const CUR_THREAD_HANDLE: u32 = 0xFFFF_8000;
const STATS_EVERY: Duration = Duration::from_secs(10);
/// How long closing a stream waits for its thread to close the renderer; the thread looks every
/// renderer frame.
const CLOSE_WAIT: Duration = Duration::from_secs(1);

/// The streams built and not yet closed. audren is one per process, so there is at most one.
static OPEN: Mutex<Vec<Arc<Shared>>> = Mutex::new(Vec::new());
static AT_EXIT: Once = Once::new();

/// libnx's `AudioRendererConfig`.
#[repr(C)]
struct AudioRendererConfig {
    output_rate: u32,
    num_voices: c_int,
    num_effects: c_int,
    num_sinks: c_int,
    num_mix_objs: c_int,
    num_mix_buffers: c_int,
}

/// The renderer SDL's Switch backend opens: 48 kHz, 24 voices, one sink, and a final mix of two
/// buffers.
const CONFIG: AudioRendererConfig = AudioRendererConfig {
    output_rate: 1, // AudioRendererOutputRate_48kHz
    num_voices: 24,
    num_effects: 0,
    num_sinks: 1,
    num_mix_objs: 1,
    num_mix_buffers: 2,
};

/// libnx's `AudioDriver`.
#[repr(C)]
struct AudioDriver {
    etc: *mut c_void,
    config: AudioRendererConfig,
    in_mempools: *mut c_void,
    in_channels: *mut u8,
    in_voices: *mut u8,
    in_mixes: *mut c_void,
    in_sinks: *mut c_void,
}

/// libnx's `AudioDriverWaveBuf`, whose `state` is an 8-bit field.
#[repr(C)]
struct WaveBuf {
    data_raw: *const c_void,
    size: u64,
    start_sample_offset: i32,
    end_sample_offset: i32,
    context_addr: *const c_void,
    context_sz: u64,
    state: u8,
    is_looping: bool,
    sequence_id: u32,
    next: *mut WaveBuf,
}

// `AudioDriverWaveBufState`: libnx sets Waiting, Queued, Playing and then Done.
const WAVEBUF_FREE: u8 = 0;
const WAVEBUF_DONE: u8 = 4;
// `AudioRendererVoicePlayState`.
const VOICE_STARTED: u8 = 0;
const VOICE_PAUSED: u8 = 2;
const PCM_INT16: c_int = 2;
const FINAL_MIX_ID: u32 = 0;
const UNUSED_SPLITTER_ID: u32 = u32::MAX;
// Byte offsets into the renderer input that libnx's inline voice helpers write and Rust can't
// call (switch/audio/driver.h): voice 0's `AudioRendererVoiceInfoIn` and an
// `AudioRendererChannelInfoIn`, checked against devkitPro's libnx with offsetof and the code GCC
// makes of those helpers.
const VOICE_STATE: usize = 0xA;
const VOICE_DEST_MIX: usize = 0x58;
const VOICE_DEST_SPLITTER: usize = 0x5C;
const VOICE_CHANNEL_IDS: usize = 0x140;
const CHANNEL_SIZE: usize = 0x70;
const CHANNEL_MIX: usize = 0x4;
static SINK_CHANNELS: [u8; 2] = [0, 1];

unsafe extern "C" {
    // libnx
    fn audrenInitialize(config: *const AudioRendererConfig) -> u32;
    fn audrenExit();
    fn audrenStartAudioRenderer() -> u32;
    fn audrenGetFrameEvent() -> *mut c_void;
    fn eventWait(event: *mut c_void, timeout: u64) -> u32;
    fn audrvCreate(
        driver: *mut AudioDriver,
        config: *const AudioRendererConfig,
        num_final_mix_channels: c_int,
    ) -> u32;
    fn audrvClose(driver: *mut AudioDriver);
    fn audrvUpdate(driver: *mut AudioDriver) -> u32;
    fn audrvMemPoolAdd(driver: *mut AudioDriver, buffer: *mut c_void, size: usize) -> c_int;
    fn audrvMemPoolAttach(driver: *mut AudioDriver, id: c_int) -> bool;
    fn audrvDeviceSinkAdd(
        driver: *mut AudioDriver,
        device_name: *const c_char,
        num_channels: c_int,
        channel_ids: *const u8,
    ) -> c_int;
    fn audrvVoiceInit(
        driver: *mut AudioDriver,
        id: c_int,
        num_channels: c_int,
        format: c_int,
        sample_rate: c_int,
    ) -> bool;
    fn audrvVoiceAddWaveBuf(driver: *mut AudioDriver, id: c_int, wavebuf: *mut WaveBuf) -> bool;
    fn armDCacheFlush(addr: *mut c_void, size: usize);
    fn svcGetThreadPriority(priority: *mut i32, handle: u32) -> u32;
    fn svcSetThreadPriority(handle: u32, priority: u32) -> u32;
    // newlib
    fn atexit(function: extern "C" fn()) -> c_int;
}

/// The renderer, its driver, and voice 0's buffers: made on the thread that builds the stream,
/// then used only by the stream's own thread.
struct Output {
    driver: AudioDriver,
    pool: *mut u8,
    layout: Layout,
    /// libnx queues these by address, so they stay in one allocation.
    bufs: Box<[WaveBuf; BUFFERS]>,
    channels: usize,
    frames: usize,
}

// SAFETY: once `Output::open` returns, only the stream's thread touches the driver, the pool and
// the buffers.
unsafe impl Send for Output {}

impl Output {
    /// Starts the renderer with voice 0 playing `channels` (1 or 2) at `rate`, paused.
    fn open(channels: usize, rate: u32, frames: usize) -> Result<Self, String> {
        let bytes = frames * channels * size_of::<i16>();
        let layout = Layout::from_size_align((bytes * BUFFERS).next_multiple_of(0x1000), 0x1000)
            .map_err(|error| format!("audio buffers: {error}"))?;
        // SAFETY: CONFIG is valid; libnx counts initialisations, and Drop's audrenExit ends this one.
        let rc = unsafe { audrenInitialize(&CONFIG) };
        if rc != 0 {
            return Err(format!("audrenInitialize failed, {rc:#x}"));
        }
        let mut driver = AudioDriver {
            etc: std::ptr::null_mut(),
            config: CONFIG,
            in_mempools: std::ptr::null_mut(),
            in_channels: std::ptr::null_mut(),
            in_voices: std::ptr::null_mut(),
            in_mixes: std::ptr::null_mut(),
            in_sinks: std::ptr::null_mut(),
        };
        // SAFETY: `driver` is writable; audrvClose frees what this allocates.
        let rc = unsafe { audrvCreate(&mut driver, &CONFIG, 2) };
        if rc != 0 {
            // SAFETY: undoes the audrenInitialize above.
            unsafe { audrenExit() };
            return Err(format!("audrvCreate failed, {rc:#x}"));
        }
        // SAFETY: the layout's size isn't zero.
        let pool = unsafe { std::alloc::alloc_zeroed(layout) };
        if pool.is_null() {
            // SAFETY: undoes audrvCreate and audrenInitialize.
            unsafe {
                audrvClose(&mut driver);
                audrenExit();
            }
            return Err(format!("no memory for {} bytes of audio buffers", layout.size()));
        }
        // SDL's layout: every buffer is the whole pool, and its sample offsets pick its part.
        let bufs = Box::new(std::array::from_fn(|i| WaveBuf {
            data_raw: pool.cast(),
            size: (bytes * BUFFERS) as u64,
            start_sample_offset: (i * frames) as i32,
            end_sample_offset: ((i + 1) * frames) as i32,
            context_addr: std::ptr::null(),
            context_sz: 0,
            state: WAVEBUF_FREE,
            is_looping: false,
            sequence_id: 0,
            next: std::ptr::null_mut(),
        }));
        let mut output = Output { driver, pool, layout, bufs, channels, frames };
        output.start(rate)?;
        Ok(output)
    }

    /// The rest of SDL's `SWITCHAUDIO_OpenDevice`, checking each step. Drop undoes it.
    fn start(&mut self, rate: u32) -> Result<(), String> {
        let driver = &mut self.driver;
        // SAFETY: the pool is page-aligned, a whole number of pages, and outlives the renderer.
        let pool = unsafe { audrvMemPoolAdd(driver, self.pool.cast(), self.layout.size()) };
        // SAFETY: plain libnx calls on the driver made in `open`.
        if pool < 0 || !unsafe { audrvMemPoolAttach(driver, pool) } {
            return Err("audrvMemPoolAdd failed".into());
        }
        // SAFETY: a NUL-terminated name and two channel ids, which libnx copies.
        if unsafe { audrvDeviceSinkAdd(driver, c"MainAudioOut".as_ptr(), 2, SINK_CHANNELS.as_ptr()) }
            < 0
        {
            return Err("audrvDeviceSinkAdd failed".into());
        }
        // SAFETY: as above.
        let rc = unsafe { audrvUpdate(driver) };
        if rc != 0 {
            return Err(format!("audrvUpdate failed, {rc:#x}"));
        }
        // SAFETY: as above.
        let rc = unsafe { audrenStartAudioRenderer() };
        if rc != 0 {
            return Err(format!("audrenStartAudioRenderer failed, {rc:#x}"));
        }
        // SAFETY: as above.
        if !unsafe { audrvVoiceInit(driver, 0, self.channels as c_int, PCM_INT16, rate as c_int) } {
            return Err("audrvVoiceInit failed".into());
        }
        // audrvVoiceSetDestinationMix to the final mix, then audrvVoiceSetMixFactor: each channel
        // to its own speaker, or a mono one to both. audrvVoiceInit zeroed the other factors.
        self.write_voice(VOICE_DEST_MIX, FINAL_MIX_ID);
        self.write_voice(VOICE_DEST_SPLITTER, UNUSED_SPLITTER_ID);
        if self.channels == 1 {
            self.set_mix_factor(0, 0, 1.0);
            self.set_mix_factor(0, 1, 1.0);
        } else {
            self.set_mix_factor(0, 0, 1.0);
            self.set_mix_factor(1, 1, 1.0);
        }
        self.set_voice_state(VOICE_PAUSED);
        Ok(())
    }

    fn write_voice(&mut self, offset: usize, value: u32) {
        // SAFETY: a u32 field of voice 0's AudioRendererVoiceInfoIn (offsets above).
        unsafe { self.driver.in_voices.add(offset).cast::<u32>().write_unaligned(value) }
    }

    /// audrvVoiceStart and audrvVoiceSetPaused.
    fn set_voice_state(&mut self, state: u8) {
        // SAFETY: voice 0's play state (offsets above).
        unsafe { self.driver.in_voices.add(VOICE_STATE).write(state) }
    }

    /// audrvVoiceSetMixFactor for voice 0.
    fn set_mix_factor(&mut self, channel: usize, speaker: usize, factor: f32) {
        // SAFETY: voice 0's channel id, then that channel's mix factor (offsets above).
        unsafe {
            let id = self.driver.in_voices.add(VOICE_CHANNEL_IDS + 4 * channel).cast::<u32>();
            let mix = self.driver.in_channels.add(
                id.read_unaligned() as usize * CHANNEL_SIZE + CHANNEL_MIX + 4 * speaker,
            );
            mix.cast::<f32>().write_unaligned(factor);
        }
    }

    fn finished(&self, buf: usize) -> bool {
        matches!(self.bufs[buf].state, WAVEBUF_FREE | WAVEBUF_DONE)
    }

    /// Converts `samples` into buffer `buf`, which the renderer has finished, and queues it.
    fn queue(&mut self, buf: usize, samples: &[f32]) -> bool {
        // SAFETY: buffer `buf`'s part of the pool, which the renderer isn't reading.
        let pcm = unsafe {
            std::slice::from_raw_parts_mut(
                self.pool.cast::<i16>().add(buf * self.frames * self.channels),
                samples.len(),
            )
        };
        for (pcm, sample) in pcm.iter_mut().zip(samples) {
            *pcm = (sample.clamp(-1.0, 1.0) * 32767.0) as i16;
        }
        // SAFETY: the renderer reads the pool from memory, so it leaves the cache first; then a
        // buffer libnx has finished with.
        unsafe {
            armDCacheFlush(pcm.as_mut_ptr().cast(), size_of_val(pcm));
            audrvVoiceAddWaveBuf(&mut self.driver, 0, &mut self.bufs[buf])
        }
    }
}

impl Drop for Output {
    fn drop(&mut self) {
        // SAFETY: closing audren's session stops the renderer reading the pool, which goes last.
        unsafe {
            audrvClose(&mut self.driver);
            audrenExit();
            std::alloc::dealloc(self.pool, self.layout);
        }
    }
}

#[derive(Default)]
pub struct Devices(bool);

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Device;

pub struct Host;

type DataCallback = Box<dyn FnMut(&mut Data, &OutputCallbackInfo) + Send>;

/// What a stream and its thread share.
struct Shared {
    playing: AtomicBool,
    stop: AtomicBool,
    /// Set by the thread once it has closed the renderer.
    closed: Mutex<bool>,
    closed_signal: Condvar,
    thread: Mutex<Option<JoinHandle<()>>>,
}

impl Shared {
    /// Stops the thread, waits up to `CLOSE_WAIT` for it to close the renderer, and joins it,
    /// which frees its stack. False if it didn't stop in time.
    fn close(&self) -> bool {
        self.stop.store(true, Ordering::Release);
        let closed = self.closed.lock().unwrap_or_else(PoisonError::into_inner);
        let (closed, _) = self
            .closed_signal
            .wait_timeout_while(closed, CLOSE_WAIT, |closed| !*closed)
            .unwrap_or_else(PoisonError::into_inner);
        if !*closed {
            return false;
        }
        drop(closed);
        if let Some(thread) = self.thread.lock().unwrap_or_else(PoisonError::into_inner).take() {
            let _ = thread.join();
        }
        true
    }
}

/// `BH_AUDIO_STATS=1`: the callbacks since the last `[Audio]` line.
struct Stats {
    period: Duration,
    since: Instant,
    last: Option<Instant>,
    calls: u32,
    busy: Duration,
    slowest: Duration,
    longest_gap: Duration,
    /// Gaps over one and a half buffers.
    late: u32,
    /// Times the thread found every buffer finished: the renderer had run out of sound.
    dry: u32,
}

impl Stats {
    fn new(period: Duration) -> Self {
        Stats {
            period,
            since: Instant::now(),
            last: None,
            calls: 0,
            busy: Duration::ZERO,
            slowest: Duration::ZERO,
            longest_gap: Duration::ZERO,
            late: 0,
            dry: 0,
        }
    }

    fn record(&mut self, start: Instant, took: Duration) {
        if let Some(last) = self.last.replace(start) {
            let gap = start - last;
            self.longest_gap = self.longest_gap.max(gap);
            self.late += u32::from(gap > self.period * 3 / 2);
        }
        self.calls += 1;
        self.busy += took;
        self.slowest = self.slowest.max(took);
        let window = start - self.since;
        if window < STATS_EVERY {
            return;
        }
        eprintln!(
            "[Audio] {:.1} s: {} callbacks of {} us, {} us average, {} us slowest, {:.1}% of a \
             core; longest gap {:.1} ms, {} over 1.5 buffers; ran dry {} times",
            window.as_secs_f64(),
            self.calls,
            self.period.as_micros(),
            (self.busy / self.calls).as_micros(),
            self.slowest.as_micros(),
            100.0 * self.busy.as_secs_f64() / window.as_secs_f64(),
            self.longest_gap.as_secs_f64() * 1e3,
            self.late,
            self.dry,
        );
        *self = Stats { last: self.last, ..Stats::new(self.period) };
    }
}

/// The stream's thread: every renderer frame, refills the buffers the renderer has finished,
/// oldest first, and sends them with an update. Closes the renderer when told to stop.
fn run(
    mut output: Output,
    mut data: DataCallback,
    shared: Arc<Shared>,
    priority: Option<u32>,
    period: Duration,
    mut stats: Option<Stats>,
) {
    if let Some(priority) = priority {
        set_priority(priority);
    }
    let mut samples = vec![0.0f32; output.frames * output.channels];
    let started = Instant::now();
    let mut next = 0;
    let mut voice = VOICE_PAUSED;
    let mut primed = false;
    let mut failed_updates = 0u32;
    // SAFETY: libnx's frame event, valid while `output` holds audren open.
    let frame = unsafe { audrenGetFrameEvent() };
    while !shared.stop.load(Ordering::Acquire) {
        let state =
            if shared.playing.load(Ordering::Acquire) { VOICE_STARTED } else { VOICE_PAUSED };
        if state != voice {
            output.set_voice_state(state);
            voice = state;
        }
        if voice == VOICE_STARTED {
            let mut refilled = 0;
            while refilled < BUFFERS && output.finished(next) {
                let ahead = (0..BUFFERS).filter(|&buf| !output.finished(buf)).count();
                let start = Instant::now();
                let now = start - started;
                let callback = StreamInstant::new(now.as_secs() as i64, now.subsec_nanos());
                let playback = callback.add(period * ahead as u32).unwrap_or(callback);
                let info = OutputCallbackInfo {
                    timestamp: OutputStreamTimestamp { callback, playback },
                };
                samples.fill(0.0);
                // SAFETY: `samples` is a live f32 buffer of that many samples.
                let mut slice = unsafe {
                    Data::from_parts(samples.as_mut_ptr().cast(), samples.len(), SampleFormat::F32)
                };
                data(&mut slice, &info);
                if let Some(stats) = stats.as_mut() {
                    stats.record(start, start.elapsed());
                }
                if !output.queue(next, &samples) {
                    eprintln!("[Audio] libnx refused buffer {next}; it stays empty");
                }
                next = (next + 1) % BUFFERS;
                refilled += 1;
            }
            if refilled == BUFFERS && primed {
                if let Some(stats) = stats.as_mut() {
                    stats.dry += 1;
                }
            }
            primed |= refilled > 0;
        }
        // SAFETY: the driver `output` owns.
        let rc = unsafe { audrvUpdate(&mut output.driver) };
        if rc != 0 {
            if failed_updates == 0 {
                eprintln!("[Audio] audrvUpdate failed, {rc:#x}; the thread keeps trying");
            }
            failed_updates = failed_updates.saturating_add(1);
        }
        // SAFETY: as above. A timeout just means another look at the buffers.
        unsafe { eventWait(frame, FRAME_WAIT_NS) };
    }
    drop(output);
    *shared.closed.lock().unwrap_or_else(PoisonError::into_inner) = true;
    shared.closed_signal.notify_all();
}

pub struct Stream {
    shared: Arc<Shared>,
}

#[derive(Clone)]
pub struct SupportedInputConfigs;
#[derive(Clone)]
pub struct SupportedOutputConfigs(Option<SupportedStreamConfigRange>);

impl Host {
    #[allow(dead_code)]
    pub fn new() -> Result<Self, crate::HostUnavailable> {
        Ok(Host)
    }
}

impl Devices {
    pub fn new() -> Result<Self, DevicesError> {
        Ok(Devices(false))
    }
}

fn default_config() -> SupportedStreamConfig {
    SupportedStreamConfig::new(
        2,
        SampleRate(SAMPLE_RATE),
        SupportedBufferSize::Range { min: MIN_FRAMES, max: MAX_FRAMES },
        SampleFormat::F32,
    )
}

/// A number from the environment, decimal or `0x` hex.
fn env_number(name: &str) -> Option<u32> {
    let value = std::env::var(name).ok()?;
    let value = value.trim();
    match value.strip_prefix("0x").or_else(|| value.strip_prefix("0X")) {
        Some(hex) => u32::from_str_radix(hex, 16).ok(),
        None => value.parse().ok(),
    }
}

/// `BH_AUDIO_PRIORITY`: None keeps the priority the thread starts with.
fn audio_priority() -> Option<u32> {
    let Ok(value) = std::env::var("BH_AUDIO_PRIORITY") else {
        return Some(DEFAULT_PRIORITY);
    };
    if value.trim() == "default" {
        return None;
    }
    let priority = env_number("BH_AUDIO_PRIORITY");
    if priority.is_none() {
        eprintln!("[Audio] BH_AUDIO_PRIORITY={value:?} is not a number; using {DEFAULT_PRIORITY:#x}");
    }
    Some(priority.unwrap_or(DEFAULT_PRIORITY))
}

impl DeviceTrait for Device {
    type SupportedInputConfigs = SupportedInputConfigs;
    type SupportedOutputConfigs = SupportedOutputConfigs;
    type Stream = Stream;

    fn name(&self) -> Result<String, DeviceNameError> {
        Ok("Nintendo Switch".to_string())
    }

    fn supported_input_configs(
        &self,
    ) -> Result<SupportedInputConfigs, SupportedStreamConfigsError> {
        Ok(SupportedInputConfigs)
    }

    fn supported_output_configs(
        &self,
    ) -> Result<SupportedOutputConfigs, SupportedStreamConfigsError> {
        let config = default_config();
        Ok(SupportedOutputConfigs(Some(SupportedStreamConfigRange::new(
            config.channels(),
            config.sample_rate(),
            config.sample_rate(),
            *config.buffer_size(),
            config.sample_format(),
        ))))
    }

    fn default_input_config(&self) -> Result<SupportedStreamConfig, DefaultStreamConfigError> {
        Err(DefaultStreamConfigError::StreamTypeNotSupported)
    }

    fn default_output_config(&self) -> Result<SupportedStreamConfig, DefaultStreamConfigError> {
        Ok(default_config())
    }

    fn build_input_stream_raw<D, E>(
        &self,
        _config: &StreamConfig,
        _sample_format: SampleFormat,
        _data_callback: D,
        _error_callback: E,
        _timeout: Option<Duration>,
    ) -> Result<Self::Stream, BuildStreamError>
    where
        D: FnMut(&Data, &InputCallbackInfo) + Send + 'static,
        E: FnMut(StreamError) + Send + 'static,
    {
        Err(BuildStreamError::StreamConfigNotSupported)
    }

    fn build_output_stream_raw<D, E>(
        &self,
        config: &StreamConfig,
        sample_format: SampleFormat,
        data_callback: D,
        _error_callback: E,
        _timeout: Option<Duration>,
    ) -> Result<Self::Stream, BuildStreamError>
    where
        D: FnMut(&mut Data, &OutputCallbackInfo) + Send + 'static,
        E: FnMut(StreamError) + Send + 'static,
    {
        if sample_format != SampleFormat::F32 || !(1..=2).contains(&config.channels) {
            return Err(BuildStreamError::StreamConfigNotSupported);
        }
        let mut open = OPEN.lock().unwrap_or_else(PoisonError::into_inner);
        if !open.is_empty() {
            return Err(BackendSpecificError {
                description: "the Switch's audio renderer takes one stream at a time".into(),
            }
            .into());
        }
        let frames = env_number("BH_AUDIO_FRAMES")
            .unwrap_or(match config.buffer_size {
                BufferSize::Fixed(n) => n,
                BufferSize::Default => DEFAULT_FRAMES,
            })
            .clamp(MIN_FRAMES, MAX_FRAMES);
        let rate = config.sample_rate.0;
        let channels = usize::from(config.channels);
        let period = Duration::from_secs_f64(f64::from(frames) / f64::from(rate.max(1)));
        let stats = std::env::var("BH_AUDIO_STATS").is_ok_and(|v| v.trim() == "1");
        let priority = audio_priority();
        let output = Output::open(channels, rate, frames as usize)
            .map_err(|description| BackendSpecificError { description })?;
        let shared = Arc::new(Shared {
            playing: AtomicBool::new(false),
            stop: AtomicBool::new(false),
            closed: Mutex::new(false),
            closed_signal: Condvar::new(),
            thread: Mutex::new(None),
        });
        let thread = std::thread::Builder::new()
            .name("bh-audio".into())
            .spawn({
                let shared = Arc::clone(&shared);
                let data: DataCallback = Box::new(data_callback);
                let stats = stats.then(|| Stats::new(period));
                move || run(output, data, shared, priority, period, stats)
            })
            .map_err(|error| BackendSpecificError { description: format!("audio thread: {error}") })?;
        *shared.thread.lock().unwrap_or_else(PoisonError::into_inner) = Some(thread);
        eprintln!(
            "[Audio] audren output: {rate} Hz, {channels} channels, f32 to 16-bit, {BUFFERS} \
             buffers of {frames} frames ({:.1} ms); audio thread priority {}{}",
            period.as_secs_f64() * 1e3,
            priority.map_or("unchanged".to_string(), |p| format!("{p:#x}")),
            if stats { "; stats every 10 s" } else { "" },
        );
        AT_EXIT.call_once(|| {
            // SAFETY: registers a plain C function; newlib keeps the pointer.
            if unsafe { atexit(close_at_exit) } != 0 {
                eprintln!("[Audio] atexit failed: the renderer stays open at exit");
            }
        });
        open.push(Arc::clone(&shared));
        Ok(Stream { shared })
    }
}

/// Closes the streams still open when the process exits (top of file).
extern "C" fn close_at_exit() {
    let open = std::mem::take(&mut *OPEN.lock().unwrap_or_else(PoisonError::into_inner));
    for shared in open {
        let start = Instant::now();
        if shared.close() {
            eprintln!(
                "[Audio] exit: closed the audio renderer in {:.1} ms, which gives audren its \
                 memory back",
                start.elapsed().as_secs_f64() * 1e3
            );
        } else {
            eprintln!(
                "[Audio] exit: the audio thread didn't stop within {} ms; audren keeps its memory",
                CLOSE_WAIT.as_millis()
            );
        }
    }
}

/// Moves the calling thread, the stream's, to `priority`.
fn set_priority(priority: u32) {
    let mut before = -1;
    // SAFETY: plain SVCs on the calling thread's pseudo-handle.
    let rc = unsafe {
        svcGetThreadPriority(&mut before, CUR_THREAD_HANDLE);
        svcSetThreadPriority(CUR_THREAD_HANDLE, priority)
    };
    if rc == 0 {
        eprintln!("[Audio] audio thread priority {before:#x} -> {priority:#x}");
    } else {
        eprintln!(
            "[Audio] audio thread stays at priority {before:#x}: \
             svcSetThreadPriority({priority:#x}) failed, {rc:#x}"
        );
    }
}

impl Drop for Stream {
    fn drop(&mut self) {
        OPEN.lock()
            .unwrap_or_else(PoisonError::into_inner)
            .retain(|shared| !Arc::ptr_eq(shared, &self.shared));
        if !self.shared.close() {
            eprintln!(
                "[Audio] the audio thread didn't stop within {} ms; audren stays open",
                CLOSE_WAIT.as_millis()
            );
        }
    }
}

impl HostTrait for Host {
    type Devices = Devices;
    type Device = Device;

    fn is_available() -> bool {
        true
    }

    fn devices(&self) -> Result<Self::Devices, DevicesError> {
        Devices::new()
    }

    fn default_input_device(&self) -> Option<Device> {
        None
    }

    fn default_output_device(&self) -> Option<Device> {
        Some(Device)
    }
}

impl StreamTrait for Stream {
    fn play(&self) -> Result<(), PlayStreamError> {
        self.shared.playing.store(true, Ordering::Release);
        Ok(())
    }

    fn pause(&self) -> Result<(), PauseStreamError> {
        self.shared.playing.store(false, Ordering::Release);
        Ok(())
    }
}

impl Iterator for Devices {
    type Item = Device;

    fn next(&mut self) -> Option<Device> {
        (!std::mem::replace(&mut self.0, true)).then_some(Device)
    }
}

impl Iterator for SupportedInputConfigs {
    type Item = SupportedStreamConfigRange;

    fn next(&mut self) -> Option<SupportedStreamConfigRange> {
        None
    }
}

impl Iterator for SupportedOutputConfigs {
    type Item = SupportedStreamConfigRange;

    fn next(&mut self) -> Option<SupportedStreamConfigRange> {
        self.0.take()
    }
}
