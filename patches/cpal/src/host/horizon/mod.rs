//! Nintendo Switch (Horizon) backend over devkitPro's SDL2 audio callback.
//!
//! One output device, f32 only. SDL converts to the device format itself.

use std::ffi::{c_char, c_int, c_void};
use std::time::{Duration, Instant};

use crate::traits::{DeviceTrait, HostTrait, StreamTrait};
use crate::{
    BackendSpecificError, BuildStreamError, BufferSize, Data, DefaultStreamConfigError,
    DeviceDescription, DeviceDescriptionBuilder, DeviceId, DeviceIdError, DeviceNameError,
    DevicesError, InputCallbackInfo, OutputCallbackInfo, OutputStreamTimestamp, PauseStreamError,
    PlayStreamError, SampleFormat, StreamConfig, StreamError, StreamInstant, SupportedBufferSize,
    SupportedStreamConfig, SupportedStreamConfigRange, SupportedStreamConfigsError,
};

const SDL_INIT_AUDIO: u32 = 0x10;
const AUDIO_F32SYS: u16 = 0x8120;
const SAMPLE_RATE: u32 = 48_000;
const MIN_FRAMES: u32 = 256;
const MAX_FRAMES: u32 = 8192;

type AudioCallback = unsafe extern "C" fn(*mut c_void, *mut u8, c_int);

#[repr(C)]
struct AudioSpec {
    freq: c_int,
    format: u16,
    channels: u8,
    silence: u8,
    samples: u16,
    padding: u16,
    size: u32,
    callback: Option<AudioCallback>,
    userdata: *mut c_void,
}

unsafe extern "C" {
    fn SDL_InitSubSystem(flags: u32) -> c_int;
    fn SDL_GetError() -> *const c_char;
    fn SDL_OpenAudioDevice(
        name: *const c_char,
        iscapture: c_int,
        desired: *const AudioSpec,
        obtained: *mut AudioSpec,
        allowed_changes: c_int,
    ) -> u32;
    fn SDL_PauseAudioDevice(dev: u32, pause_on: c_int);
    fn SDL_CloseAudioDevice(dev: u32);
}

fn sdl_error(what: &str) -> BuildStreamError {
    // SAFETY: SDL_GetError returns a valid NUL-terminated string.
    let msg = unsafe { std::ffi::CStr::from_ptr(SDL_GetError()) }.to_string_lossy();
    BackendSpecificError { description: format!("{what}: {msg}") }.into()
}

#[derive(Default)]
pub struct Devices(bool);

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Device;

pub struct Host;

type DataCallback = Box<dyn FnMut(&mut Data, &OutputCallbackInfo) + Send>;

struct CallbackState {
    data: DataCallback,
    started: Instant,
}

pub struct Stream {
    device: u32,
    state: *mut CallbackState,
}

// SAFETY: `state` is only dereferenced by the SDL audio thread and freed in
// Drop after the device is closed; the SDL device id is a plain integer.
unsafe impl Send for Stream {}
unsafe impl Sync for Stream {}

crate::assert_stream_send!(Stream);
crate::assert_stream_sync!(Stream);

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
        SAMPLE_RATE,
        SupportedBufferSize::Range { min: MIN_FRAMES, max: MAX_FRAMES },
        SampleFormat::F32,
    )
}

impl DeviceTrait for Device {
    type SupportedInputConfigs = SupportedInputConfigs;
    type SupportedOutputConfigs = SupportedOutputConfigs;
    type Stream = Stream;

    fn name(&self) -> Result<String, DeviceNameError> {
        Ok("Nintendo Switch".to_string())
    }

    fn description(&self) -> Result<DeviceDescription, DeviceNameError> {
        Ok(DeviceDescriptionBuilder::new("Nintendo Switch".to_string())
            .driver("sdl2".to_string())
            .build())
    }

    fn id(&self) -> Result<DeviceId, DeviceIdError> {
        Ok(DeviceId(crate::platform::HostId::Horizon, String::new()))
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
        if sample_format != SampleFormat::F32 || config.channels == 0 || config.channels > 8 {
            return Err(BuildStreamError::StreamConfigNotSupported);
        }
        // SAFETY: plain FFI; SDL_InitSubSystem is idempotent.
        if unsafe { SDL_InitSubSystem(SDL_INIT_AUDIO) } != 0 {
            return Err(sdl_error("SDL audio init"));
        }
        let frames = match config.buffer_size {
            BufferSize::Fixed(n) => n.clamp(MIN_FRAMES, MAX_FRAMES),
            BufferSize::Default => 1024,
        };
        let state = Box::into_raw(Box::new(CallbackState {
            data: Box::new(data_callback),
            started: Instant::now(),
        }));
        let desired = AudioSpec {
            freq: config.sample_rate as c_int,
            format: AUDIO_F32SYS,
            channels: config.channels as u8,
            silence: 0,
            samples: frames as u16,
            padding: 0,
            size: 0,
            callback: Some(audio_callback),
            userdata: state.cast(),
        };
        // SAFETY: `desired` is fully initialised; allowed_changes = 0 makes SDL
        // convert to exactly what was asked for, so no `obtained` is needed.
        let device =
            unsafe { SDL_OpenAudioDevice(std::ptr::null(), 0, &desired, std::ptr::null_mut(), 0) };
        if device == 0 {
            // SAFETY: SDL never took ownership of `state`.
            drop(unsafe { Box::from_raw(state) });
            return Err(sdl_error("SDL_OpenAudioDevice"));
        }
        Ok(Stream { device, state })
    }
}

unsafe extern "C" fn audio_callback(userdata: *mut c_void, stream: *mut u8, len: c_int) {
    // SAFETY: userdata is the CallbackState leaked in build_output_stream_raw;
    // it lives until Stream::drop closes the device (which joins this thread).
    let state = unsafe { &mut *userdata.cast::<CallbackState>() };
    let now = state.started.elapsed();
    let at = StreamInstant::new(now.as_secs() as i64, now.subsec_nanos());
    let info = OutputCallbackInfo::new(OutputStreamTimestamp { callback: at, playback: at });
    // SAFETY: SDL hands a writable buffer of `len` bytes, f32-aligned.
    let mut data =
        unsafe { Data::from_parts(stream.cast(), len as usize / size_of::<f32>(), SampleFormat::F32) };
    (state.data)(&mut data, &info);
}

impl Drop for Stream {
    fn drop(&mut self) {
        // SAFETY: closing the device stops and joins the audio thread, after
        // which nothing references `state`.
        unsafe {
            SDL_CloseAudioDevice(self.device);
            drop(Box::from_raw(self.state));
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
        // SAFETY: plain FFI on an open device id.
        unsafe { SDL_PauseAudioDevice(self.device, 0) };
        Ok(())
    }

    fn pause(&self) -> Result<(), PauseStreamError> {
        // SAFETY: plain FFI on an open device id.
        unsafe { SDL_PauseAudioDevice(self.device, 1) };
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
