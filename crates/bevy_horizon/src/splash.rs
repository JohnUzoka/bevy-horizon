//! Something on screen from the first moment of `main`, before Bevy's renderer has a surface.
//!
//! A game's own start (reading its data, Bevy's plugins, its Startup systems) can take many
//! seconds before the first frame, and the Switch shows a black screen meanwhile; a retail game
//! shows its start-up logo at once. [`show`] puts a libnx framebuffer on the default `NWindow` and
//! redraws it from a thread of its own with the game's drawing function. The primary window's
//! attachment (`window.rs`) calls [`close`] just before wgpu takes the same `NWindow` for its
//! swapchain, so the game's first frame replaces the splash; a game that [`hold`]s it keeps it
//! up, without drawing to the screen itself, until it lets go.
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

/// How long the splash waits after its last frame for the screen to take it: three vsyncs.
const DRAINED: Duration = Duration::from_millis(50);

unsafe extern "C" {
    fn svcSetThreadPriority(handle: u32, priority: u32) -> u32;
    fn nwindowGetDefault() -> *mut c_void;
    fn framebufferCreate(fb: *mut Framebuffer, win: *mut c_void, width: u32, height: u32, format: u32, num_fbs: u32) -> u32;
    fn framebufferMakeLinear(fb: *mut Framebuffer) -> u32;
    fn framebufferClose(fb: *mut Framebuffer);
    fn framebufferBegin(fb: *mut Framebuffer, out_stride: *mut u32) -> *mut c_void;
    fn framebufferEnd(fb: *mut Framebuffer);
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
    let spawned = std::thread::Builder::new().name("splash".into()).spawn(move || {
        // SAFETY: changes only this thread's priority.
        let priority = unsafe { svcSetThreadPriority(CUR_THREAD_HANDLE, PRIORITY) };
        let mut fb = Framebuffer([0; 128]);
        // SAFETY: libnx creates the framebuffer in `fb`, on the window it made before `main`.
        // Three buffers, as NVK's swapchain has; `[Window] producer` lines in log.txt say how many
        // each producer used.
        let rc = unsafe { framebufferCreate(&mut fb, nwindowGetDefault(), width, height, PIXEL_FORMAT_RGBA_8888, 3) };
        if rc != 0 {
            let _ = made.send(Err(rc));
            return;
        }
        // SAFETY: `fb` was just created.
        let rc = unsafe { framebufferMakeLinear(&mut fb) };
        if rc != 0 {
            // SAFETY: as above; this releases the window's buffers again.
            unsafe { framebufferClose(&mut fb) };
            let _ = made.send(Err(rc));
            return;
        }
        let _ = made.send(Ok(priority));
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
        // Android's disconnect frees the window's slots but keeps its queue, so a frame still queued
        // when the buffers are released could go to the screen after the swapchain's buffers are on
        // the window, and keep a slot the swapchain then never gets back.
        std::thread::sleep(DRAINED);
        // SAFETY: nothing draws into `fb` any more. This releases the window's buffers, so another
        // producer (wgpu's swapchain) can configure the window.
        unsafe { framebufferClose(&mut fb) };
    });
    let thread = match spawned {
        Ok(thread) => thread,
        Err(error) => {
            tracing::warn!("splash: no thread ({error}); the screen stays black until the first frame");
            return false;
        }
    };
    match ready.recv() {
        Ok(Ok(priority)) => {
            if priority != 0 {
                tracing::warn!("splash: svcSetThreadPriority({PRIORITY:#x}) failed (0x{priority:x}); a busy core can hold its frames back");
            }
            *running = Some(Running { stop, thread });
            true
        }
        Ok(Err(rc)) => {
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
/// Releasing the framebuffer leaves libnx's window at pixel format 0, which the
/// `nwindowConfigureBuffer` wrapper (`compat.rs`) undoes before the swapchain's buffers go on it.
/// True when a splash was up.
pub fn close() -> bool {
    let running = RUNNING.lock().unwrap_or_else(PoisonError::into_inner).take();
    let Some(Running { stop, thread }) = running else {
        return false;
    };
    stop.store(true, Ordering::Release);
    if thread.join().is_err() {
        tracing::warn!("splash: its thread panicked");
    }
    tracing::info!("splash closed: the game's first frame replaces it");
    true
}

/// Whether the splash is up.
pub fn shown() -> bool {
    RUNNING.lock().unwrap_or_else(PoisonError::into_inner).is_some()
}
