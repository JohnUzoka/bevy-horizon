//! Something on screen from the first moment of `main`, before Bevy's renderer has a surface.
//!
//! A game's own start (reading its data, Bevy's plugins, its Startup systems) can take many
//! seconds before the first frame, and the Switch shows a black screen meanwhile; a retail game
//! shows its start-up logo at once. [`show`] puts a libnx framebuffer on the default `NWindow` and
//! redraws it from a thread of its own with the game's drawing function. The primary window's
//! attachment (`window.rs`) calls [`close`], then takes [`game_window`] for wgpu's swapchain, so
//! the game's first frame replaces the splash; a game that [`hold`]s it keeps it up, without
//! drawing to the screen itself, until it lets go.
//!
//! The game's window is a display layer of its own, opened after the splash: with the swapchain on
//! the `NWindow` the splash's framebuffer had used, Skate 3 ran at 20 fps in game where it ran at
//! 33.7 without the splash, whatever was put back on the window afterwards (sk8 findings 59
//! §14-18). Opened later, its layer is above the splash's, whose last frame is black.
//! `BH_SPLASH_LAYERS` picks another arrangement: `splash` puts the splash on a layer of its own,
//! closed when it closes, and leaves the default `NWindow` to the game; `shared` puts both on
//! the default `NWindow`, as before.
use core::ffi::c_void;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// libnx's `Framebuffer` (display/framebuffer.h), 80 bytes on AArch64; only libnx reads it.
#[repr(C, align(8))]
struct Framebuffer([u8; 128]);

/// libnx `PIXEL_FORMAT_RGBA_8888` (display/types.h).
const PIXEL_FORMAT_RGBA_8888: u32 = 1;

const CUR_THREAD_HANDLE: u32 = 0xFFFF_8000;
/// The splash thread's priority: just above the main thread's 0x2C, and so above every pthread's
/// 0x3B. Horizon never time-slices a lower-priority thread in while a higher one is runnable on
/// its core, so at 0x3B the splash's frames waited whenever a start or a loader kept its core
/// busy, and the board stuttered. It sleeps between frames, so it takes little from them.
const PRIORITY: u32 = 0x2B;

/// libnx's `ViDisplay`, `ViLayer` and `NWindow` (services/vi.h, display/native_window.h): 80, 16
/// and 128 bytes, 8-aligned, from devkitA64's libnx headers; only libnx reads them.
#[repr(C, align(8))]
struct ViDisplay([u8; 80]);
#[repr(C, align(8))]
struct ViLayer([u8; 16]);
#[repr(C, align(8))]
struct NWindow([u8; 128]);

/// libnx `ViScalingMode_FitToLayer`, as its default window uses.
const VI_SCALING_MODE_FIT_TO_LAYER: u32 = 2;

unsafe extern "C" {
    fn svcSetThreadPriority(handle: u32, priority: u32) -> u32;
    fn nwindowGetDefault() -> *mut c_void;
    fn viCreateLayer(display: *const ViDisplay, layer: *mut ViLayer) -> u32;
    fn viSetLayerScalingMode(layer: *mut ViLayer, scaling_mode: u32) -> u32;
    fn viCloseLayer(layer: *mut ViLayer) -> u32;
    fn nwindowCreateFromLayer(nw: *mut NWindow, layer: *const ViLayer) -> u32;
    fn nwindowClose(nw: *mut NWindow);
    fn framebufferCreate(fb: *mut Framebuffer, win: *mut c_void, width: u32, height: u32, format: u32, num_fbs: u32) -> u32;
    fn framebufferMakeLinear(fb: *mut Framebuffer) -> u32;
    fn framebufferClose(fb: *mut Framebuffer);
    fn framebufferBegin(fb: *mut Framebuffer, out_stride: *mut u32) -> *mut c_void;
    fn framebufferEnd(fb: *mut Framebuffer);
}

/// Which layer the splash and the game draw on (`BH_SPLASH_LAYERS`).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Layers {
    /// The splash on the default `NWindow`, the game on a layer of its own.
    Game,
    /// The splash on a layer of its own, the game on the default `NWindow`.
    Splash,
    /// Both on the default `NWindow`.
    Shared,
}

fn layers() -> Layers {
    match std::env::var("BH_SPLASH_LAYERS").as_deref().map(str::trim) {
        Ok("splash") => Layers::Splash,
        Ok("shared") => Layers::Shared,
        _ => Layers::Game,
    }
}

/// Whether the splash's framebuffer has been on the default `NWindow`, so the game is better off
/// on a layer of its own (`game_window`).
static USED_DEFAULT_WINDOW: AtomicBool = AtomicBool::new(false);

/// A display layer and the `NWindow` on it, closed when dropped, on the default display that
/// libnx's default window opened.
struct OwnLayer {
    _display: Box<ViDisplay>,
    layer: Box<ViLayer>,
    window: Box<NWindow>,
}

impl OwnLayer {
    /// Opens a layer as libnx's default window does (`__nx_win_init`), on the default display.
    fn open() -> Result<Self, (&'static str, u32)> {
        // Not opened again: libnx's default window has the display open, and a second
        // `viOpenDisplay` fails (vi `AlreadyOpened`, 0x1272). Opening a layer reads only the
        // display's name and that it is open (libnx `_viOpenLayer`): `display_id` at 0, the name at
        // 8 and `initialized` at 0x48.
        let mut display = Box::new(ViDisplay([0; 80]));
        display.0[8..8 + 7].copy_from_slice(b"Default");
        display.0[0x48] = 1;
        let mut layer = Box::new(ViLayer([0; 16]));
        // SAFETY: each call gets the libnx struct it fills, alive in its box as long as `Self`.
        let rc = unsafe { viCreateLayer(&*display, &mut *layer) };
        if rc != 0 {
            return Err(("viCreateLayer", rc));
        }
        let rc = unsafe { viSetLayerScalingMode(&mut *layer, VI_SCALING_MODE_FIT_TO_LAYER) };
        let mut window = Box::new(NWindow([0; 128]));
        let rc = if rc != 0 { Err(("viSetLayerScalingMode", rc)) } else {
            match unsafe { nwindowCreateFromLayer(&mut *window, &*layer) } {
                0 => Ok(()),
                rc => Err(("nwindowCreateFromLayer", rc)),
            }
        };
        if let Err(failure) = rc {
            unsafe { viCloseLayer(&mut *layer) };
            return Err(failure);
        }
        Ok(Self { _display: display, layer, window })
    }

    fn window(&mut self) -> *mut c_void {
        (&mut *self.window as *mut NWindow).cast()
    }
}

impl Drop for OwnLayer {
    fn drop(&mut self) {
        // SAFETY: the framebuffer on the window is closed first (the splash thread's end).
        unsafe {
            nwindowClose(&mut *self.window);
            viCloseLayer(&mut *self.layer);
        }
    }
}

/// Why the splash couldn't go up.
enum Failure {
    Layer(&'static str, u32),
    Framebuffer(u32),
}

/// One frame to draw: `height` rows of RGBA8 pixels, `stride` bytes apart.
pub struct Frame<'a> {
    pub pixels: &'a mut [u8],
    pub width: u32,
    pub height: u32,
    pub stride: usize,
    /// Since [`show`].
    pub elapsed: Duration,
}

struct Running {
    stop: Arc<AtomicBool>,
    thread: JoinHandle<()>,
}

static RUNNING: Mutex<Option<Running>> = Mutex::new(None);
static HELD: AtomicBool = AtomicBool::new(false);

/// Keeps the splash up past the first frame. While it is held, `window.rs` doesn't attach the
/// primary window, so the game runs without drawing to the screen and the splash stays;
/// `hold(false)` lets the next frame attach the window and close the splash.
pub fn hold(held: bool) {
    HELD.store(held, Ordering::Release);
}

/// Whether the splash is up and holding the screen.
pub fn held() -> bool {
    HELD.load(Ordering::Acquire) && shown()
}

/// Puts up the splash: a `width` × `height` framebuffer (the display scales it to the screen),
/// with `draw` called on the splash's thread about every `interval`. False when libnx couldn't
/// make the framebuffer; the screen then stays black until the first frame, as without it.
pub fn show(width: u32, height: u32, interval: Duration, mut draw: impl FnMut(Frame<'_>) + Send + 'static) -> bool {
    let mut running = RUNNING.lock().unwrap_or_else(PoisonError::into_inner);
    if running.is_some() {
        return true;
    }
    let stop = Arc::new(AtomicBool::new(false));
    let stopping = stop.clone();
    let (made, ready) = std::sync::mpsc::channel();
    let default_window = layers() != Layers::Splash;
    let spawned = std::thread::Builder::new().name("splash".into()).spawn(move || {
        // SAFETY: changes only this thread's priority.
        let priority = unsafe { svcSetThreadPriority(CUR_THREAD_HANDLE, PRIORITY) };
        let mut own = if default_window {
            None
        } else {
            match OwnLayer::open() {
                Ok(layer) => Some(layer),
                Err((call, rc)) => {
                    let _ = made.send(Err(Failure::Layer(call, rc)));
                    return;
                }
            }
        };
        // SAFETY: libnx's default window lives as long as the process.
        let window = own.as_mut().map_or_else(|| unsafe { nwindowGetDefault() }, OwnLayer::window);
        let mut fb = Framebuffer([0; 128]);
        // SAFETY: libnx creates the framebuffer in `fb`, on `window`, which outlives it. Three
        // buffers, as NVK's swapchain has.
        let rc = unsafe { framebufferCreate(&mut fb, window, width, height, PIXEL_FORMAT_RGBA_8888, 3) };
        if rc != 0 {
            let _ = made.send(Err(Failure::Framebuffer(rc)));
            return;
        }
        // SAFETY: `fb` was just created.
        let rc = unsafe { framebufferMakeLinear(&mut fb) };
        if rc != 0 {
            // SAFETY: as above; this releases the window's buffers again.
            unsafe { framebufferClose(&mut fb) };
            let _ = made.send(Err(Failure::Framebuffer(rc)));
            return;
        }
        if own.is_none() {
            USED_DEFAULT_WINDOW.store(true, Ordering::Release);
        }
        let _ = made.send(Ok((priority, own.is_some())));
        let started = Instant::now();
        while !stopping.load(Ordering::Acquire) {
            let frame_started = Instant::now();
            let mut stride = 0u32;
            // SAFETY: paired with the `framebufferEnd` below; the linear buffer is `stride` bytes
            // a row for `height` rows and stays valid until then.
            let pixels = unsafe { framebufferBegin(&mut fb, &mut stride) };
            if !pixels.is_null() {
                let pixels = unsafe { std::slice::from_raw_parts_mut(pixels.cast::<u8>(), stride as usize * height as usize) };
                draw(Frame { pixels, width, height, stride: stride as usize, elapsed: started.elapsed() });
            }
            // SAFETY: as above.
            unsafe { framebufferEnd(&mut fb) };
            if let Some(rest) = interval.checked_sub(frame_started.elapsed()) {
                std::thread::sleep(rest);
            }
        }
        // Its last frame black: under the game's layer it stays on screen, and nothing of it may
        // show through.
        if own.is_none() && layers() == Layers::Game {
            let mut stride = 0u32;
            // SAFETY: as in the loop.
            let pixels = unsafe { framebufferBegin(&mut fb, &mut stride) };
            if !pixels.is_null() {
                let pixels = unsafe { std::slice::from_raw_parts_mut(pixels.cast::<u8>(), stride as usize * height as usize) };
                for pixel in pixels.chunks_exact_mut(4) {
                    pixel.copy_from_slice(&[0, 0, 0, 255]);
                }
            }
            unsafe { framebufferEnd(&mut fb) };
        }
        // SAFETY: nothing draws into `fb` any more. This releases the window's buffers, so another
        // producer (wgpu's swapchain) can configure the window, and then the layer goes.
        unsafe { framebufferClose(&mut fb) };
        drop(own);
    });
    let thread = match spawned {
        Ok(thread) => thread,
        Err(error) => {
            tracing::warn!("splash: no thread ({error}); the screen stays black until the first frame");
            return false;
        }
    };
    match ready.recv() {
        Ok(Ok((priority, own_layer))) => {
            if priority != 0 {
                tracing::warn!("splash: svcSetThreadPriority({PRIORITY:#x}) failed (0x{priority:x}); a busy core can hold its frames back");
            }
            tracing::info!(
                "splash: on {}",
                if own_layer { "a display layer of its own (BH_SPLASH_LAYERS=splash)" } else { "the default window" }
            );
            *running = Some(Running { stop, thread });
            true
        }
        Ok(Err(Failure::Layer(call, rc))) => {
            let _ = thread.join();
            tracing::warn!("splash: no display layer of its own ({call}: 0x{rc:x}); the screen stays black until the first frame");
            false
        }
        Ok(Err(Failure::Framebuffer(rc))) => {
            let _ = thread.join();
            tracing::warn!("splash: libnx couldn't make the framebuffer (0x{rc:x}); the screen stays black until the first frame");
            false
        }
        Err(_) => {
            let _ = thread.join();
            false
        }
    }
}

/// Takes the splash down and gives the window back; waits for its thread. Does nothing without one.
/// On the default window (`BH_SPLASH_LAYER=default`), releasing the framebuffer leaves it at
/// pixel format 0, which the `nwindowConfigureBuffer` wrapper (`compat.rs`) undoes.
pub fn close() {
    let running = RUNNING.lock().unwrap_or_else(PoisonError::into_inner).take();
    if let Some(Running { stop, thread }) = running {
        stop.store(true, Ordering::Release);
        if thread.join().is_err() {
            tracing::warn!("splash: its thread panicked");
        }
        tracing::info!("splash closed: the game's first frame replaces it");
    }
}

/// The game's display layer (`game_window`), from its opening until the NRO ends
/// (`release_game_layer`, a release hook).
static GAME_LAYER: Mutex<Option<OwnLayer>> = Mutex::new(None);

fn release_game_layer() {
    drop(GAME_LAYER.lock().unwrap_or_else(PoisonError::into_inner).take());
}

/// The `NWindow` for the game's swapchain, after [`close`]: a display layer of its own once the
/// splash has been on the default `NWindow`, unless `BH_SPLASH_LAYERS` says otherwise or no layer
/// can be opened; the default `NWindow` otherwise. The layer stays open until the NRO ends.
pub fn game_window() -> *mut c_void {
    // SAFETY: libnx's default window lives as long as the process.
    let default = unsafe { nwindowGetDefault() };
    if !USED_DEFAULT_WINDOW.load(Ordering::Acquire) || layers() != Layers::Game {
        return default;
    }
    match OwnLayer::open() {
        Ok(layer) => {
            tracing::info!("splash: the game draws on a display layer of its own, not on the window the splash used");
            crate::lifecycle::register_release_hook(release_game_layer);
            GAME_LAYER.lock().unwrap_or_else(PoisonError::into_inner).insert(layer).window()
        }
        Err((call, rc)) => {
            tracing::warn!("splash: no display layer for the game ({call}: 0x{rc:x}); it draws on the window the splash used");
            default
        }
    }
}

/// Whether the splash is up.
pub fn shown() -> bool {
    RUNNING.lock().unwrap_or_else(PoisonError::into_inner).is_some()
}
