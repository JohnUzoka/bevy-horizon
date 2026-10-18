//! libnx bindings the captest calls directly. Layouts are from libnx's headers (doc 12 lists the
//! sources): `PadState` 56 bytes, `HidTouchScreenState` 656, `HidSixAxisSensorState` 96.

#![allow(non_snake_case, dead_code)]

use core::ffi::{c_char, c_int, c_void};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

pub const CUR_THREAD: u32 = 0xFFFF_8000;
pub const CUR_PROCESS: u32 = 0xFFFF_8001;

pub const BTN_A: u64 = 1 << 0;
pub const BTN_B: u64 = 1 << 1;
pub const BTN_X: u64 = 1 << 2;
pub const BTN_Y: u64 = 1 << 3;
pub const BTN_L: u64 = 1 << 6;
pub const BTN_R: u64 = 1 << 7;
pub const BTN_PLUS: u64 = 1 << 10;
pub const BTN_MINUS: u64 = 1 << 11;

pub const NPAD_ID_NO1: u32 = 0;
pub const NPAD_ID_HANDHELD: u32 = 0x20;
pub const STYLE_FULLKEY: u32 = 1 << 0;
pub const STYLE_HANDHELD: u32 = 1 << 1;
pub const STYLE_SET_STANDARD: u32 = 0x1F;

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct StickState {
    pub x: i32,
    pub y: i32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct PadState {
    pub id_mask: u8,
    pub active_id_mask: u8,
    pub read_handheld: bool,
    pub active_handheld: bool,
    pub style_set: u32,
    pub attributes: u32,
    _pad: u32,
    pub buttons_cur: u64,
    pub buttons_old: u64,
    pub sticks: [StickState; 2],
    pub gc_triggers: [u32; 2],
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct TouchState {
    pub delta_time: u64,
    pub attributes: u32,
    pub finger_id: u32,
    pub x: u32,
    pub y: u32,
    pub diameter_x: u32,
    pub diameter_y: u32,
    pub rotation_angle: u32,
    pub reserved: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct TouchScreenState {
    pub sampling_number: u64,
    pub count: i32,
    pub reserved: u32,
    pub touches: [TouchState; 16],
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct Vec3f {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

impl Vec3f {
    pub fn len(&self) -> f32 {
        (self.x * self.x + self.y * self.y + self.z * self.z).sqrt()
    }
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct SixAxisState {
    pub delta_time: u64,
    pub sampling_number: u64,
    pub acceleration: Vec3f,
    pub angular_velocity: Vec3f,
    pub angle: Vec3f,
    pub direction: [f32; 9],
    pub attributes: u32,
    pub reserved: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct VibrationValue {
    pub amp_low: f32,
    pub freq_low: f32,
    pub amp_high: f32,
    pub freq_high: f32,
}

#[repr(C)]
pub struct AppletHookCookie {
    next: *mut AppletHookCookie,
    callback: Option<unsafe extern "C" fn(u32, *mut c_void)>,
    param: *mut c_void,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct Mallinfo {
    pub arena: usize,
    pub ordblks: usize,
    pub smblks: usize,
    pub hblks: usize,
    pub hblkhd: usize,
    pub usmblks: usize,
    pub fsmblks: usize,
    pub uordblks: usize,
    pub fordblks: usize,
    pub keepcost: usize,
}

#[repr(C, align(8))]
pub struct ClkrstSession(pub [u8; 32]);

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct MemoryInfo {
    pub addr: u64,
    pub size: u64,
    pub kind: u32,
    pub attr: u32,
    pub perm: u32,
    pub ipc_refcount: u32,
    pub device_refcount: u32,
    pub padding: u32,
}

unsafe extern "C" {
    pub fn padConfigureInput(max_players: u32, style_set: u32);
    pub fn padInitializeWithMask(pad: *mut PadState, mask: u64);
    pub fn padUpdate(pad: *mut PadState);
    pub fn hidInitializeTouchScreen();
    pub fn hidGetTouchScreenStates(states: *mut TouchScreenState, count: usize) -> usize;
    pub fn hidGetSixAxisSensorHandles(handles: *mut u32, total: i32, id: u32, style: u32) -> u32;
    pub fn hidStartSixAxisSensor(handle: u32) -> u32;
    pub fn hidStopSixAxisSensor(handle: u32) -> u32;
    pub fn hidGetSixAxisSensorStates(handle: u32, states: *mut SixAxisState, count: usize)
    -> usize;
    pub fn hidInitializeVibrationDevices(handles: *mut u32, total: i32, id: u32, style: u32)
    -> u32;
    pub fn hidSendVibrationValues(
        handles: *const u32,
        values: *const VibrationValue,
        count: i32,
    ) -> u32;
    fn setsysInitialize() -> u32;
    fn setsysExit();
    fn setsysGetProductModel(model: *mut i32) -> u32;

    pub fn appletMainLoop() -> bool;
    pub fn appletLockExit() -> u32;
    pub fn appletUnlockExit() -> u32;
    fn appletHook(
        cookie: *mut AppletHookCookie,
        callback: unsafe extern "C" fn(u32, *mut c_void),
        param: *mut c_void,
    );
    pub fn appletGetFocusState() -> u32;
    pub fn appletGetAppletType() -> i32;
    pub fn appletGetOperationMode() -> u32;
    pub fn appletGetPerformanceMode() -> i32;
    pub fn appletSetCpuBoostMode(mode: u32) -> u32;

    pub fn hosversionGet() -> u32;
    pub fn hosversionIsAtmosphere() -> bool;

    pub fn clkrstInitialize() -> u32;
    pub fn clkrstExit();
    pub fn clkrstOpenSession(session: *mut ClkrstSession, module_id: u32, unk: u32) -> u32;
    pub fn clkrstGetClockRate(session: *mut ClkrstSession, out_hz: *mut u32) -> u32;
    pub fn clkrstCloseSession(session: *mut ClkrstSession);

    pub fn nifmInitialize(service_type: u32) -> u32;
    pub fn nifmExit();
    pub fn nifmGetInternetConnectionStatus(
        kind: *mut u32,
        strength: *mut u32,
        status: *mut u32,
    ) -> u32;
    pub fn nifmGetCurrentIpAddress(out: *mut u32) -> u32;

    pub fn svcGetInfo(out: *mut u64, id0: u32, handle: u32, id1: u64) -> u32;
    pub fn svcSleepThread(nano: i64);
    pub fn svcGetCurrentProcessorNumber() -> u32;
    pub fn svcSetThreadPriority(handle: u32, priority: u32) -> u32;
    pub fn svcGetThreadPriority(out: *mut i32, handle: u32) -> u32;
    pub fn svcSetThreadCoreMask(handle: u32, core: i32, mask: u64) -> u32;
    pub fn svcGetThreadCoreMask(core: *mut i32, mask: *mut u64, handle: u32) -> u32;
    pub fn svcQueryMemory(info: *mut MemoryInfo, page_info: *mut u32, addr: u64) -> u32;
    pub fn svcExitProcess() -> !;

    pub fn consoleInit(console: *mut c_void) -> *mut c_void;
    pub fn consoleUpdate(console: *mut c_void);
    pub fn consoleExit(console: *mut c_void);

    pub fn mallinfo() -> Mallinfo;
    pub fn statvfs(path: *const c_char, buf: *mut u64) -> c_int;
    pub fn _exit(code: c_int) -> !;
    pub fn write(fd: c_int, buf: *const c_void, len: usize) -> isize;

    static devoptab_list: [*const u8; 35];
    fn __getreent() -> *mut c_void;
}

/// `SetSysProductModel_Hoag`, the Switch Lite (libnx `services/set.h`).
pub const PRODUCT_MODEL_HOAG: i32 = 4;

/// Which Switch this is, from set:sys.
pub fn product_model() -> Result<i32, u32> {
    unsafe {
        let rc = setsysInitialize();
        if rc != 0 {
            return Err(rc);
        }
        let mut model = 0;
        let rc = setsysGetProductModel(&mut model);
        setsysExit();
        if rc == 0 { Ok(model) } else { Err(rc) }
    }
}

pub fn core_now() -> u32 {
    unsafe { svcGetCurrentProcessorNumber() }
}

pub fn info(id0: u32, id1: u64) -> Option<u64> {
    let mut out = 0u64;
    let rc = unsafe { svcGetInfo(&mut out, id0, CUR_PROCESS, id1) };
    (rc == 0).then_some(out)
}

pub fn heap_in_use() -> usize {
    unsafe { mallinfo() }.uordblks
}

/// The calling thread's stack mapping: (bytes still free below this frame, total size).
#[inline(never)]
pub fn stack_here() -> Option<(u64, u64)> {
    let marker = 0u8;
    let addr = core::hint::black_box(&marker) as *const u8 as u64;
    let mut mi = MemoryInfo::default();
    let mut page = 0u32;
    let rc = unsafe { svcQueryMemory(&mut mi, &mut page, addr) };
    (rc == 0).then(|| (addr - mi.addr, mi.size))
}

pub fn core_mask_here() -> (i32, u64) {
    let (mut core, mut mask) = (-1i32, 0u64);
    unsafe { svcGetThreadCoreMask(&mut core, &mut mask, CUR_THREAD) };
    (core, mask)
}

pub fn ticks() -> u64 {
    let v: u64;
    unsafe { core::arch::asm!("mrs {}, cntpct_el0", out(reg) v) };
    v
}

pub fn tick_freq() -> u64 {
    let v: u64;
    unsafe { core::arch::asm!("mrs {}, cntfrq_el0", out(reg) v) };
    v
}

pub fn hos_version() -> String {
    let v = unsafe { hosversionGet() };
    let ams = if unsafe { hosversionIsAtmosphere() } {
        ", Atmosphere"
    } else {
        ""
    };
    format!("{}.{}.{}{ams}", v >> 16, (v >> 8) & 0xFF, v & 0xFF)
}

pub fn rc_text(rc: u32) -> String {
    format!(
        "{rc:#x} ({:04}-{:04})",
        2000 + (rc & 0x1FF),
        (rc >> 9) & 0x1FFF
    )
}

// ---- Console. fd 1 stays on log.txt (bevy_horizon dup2s it there); consoleInit points devoptab
// slot 1 at the console, so the screen is written through that slot directly.

static CONSOLE_ON: AtomicBool = AtomicBool::new(false);

/// False if the console couldn't take the default window (consoleInit returns NULL then).
pub fn console_init() -> bool {
    let ok = !unsafe { consoleInit(core::ptr::null_mut()) }.is_null();
    CONSOLE_ON.store(ok, Ordering::SeqCst);
    ok
}

pub fn console_exit() {
    if CONSOLE_ON.swap(false, Ordering::SeqCst) {
        unsafe { consoleExit(core::ptr::null_mut()) };
    }
}

pub fn console_on() -> bool {
    CONSOLE_ON.load(Ordering::SeqCst)
}

/// Callers serialise screen access (report.rs holds its lock); libnx's console isn't thread-safe.
pub fn console_write(bytes: &[u8]) {
    if !console_on() || bytes.is_empty() {
        return;
    }
    type WriteR = unsafe extern "C" fn(*mut c_void, *mut c_void, *const c_char, usize) -> isize;
    unsafe {
        let slot = core::ptr::read_volatile((&raw const devoptab_list).cast::<*const u8>().add(1));
        if slot.is_null() {
            return;
        }
        let write_r = core::ptr::read(slot.add(32).cast::<Option<WriteR>>());
        if let Some(write_r) = write_r {
            write_r(
                __getreent(),
                core::ptr::null_mut(),
                bytes.as_ptr().cast(),
                bytes.len(),
            );
        }
    }
}

pub fn console_update() {
    if console_on() {
        unsafe { consoleUpdate(core::ptr::null_mut()) };
    }
}

// ---- Applet hook: counts focus changes and remembers an exit request.

pub static FOCUS_LOST: AtomicU32 = AtomicU32::new(0);
pub static FOCUS_GAINED: AtomicU32 = AtomicU32::new(0);
pub static RESUMED: AtomicU32 = AtomicU32::new(0);
pub static MODE_CHANGES: AtomicU32 = AtomicU32::new(0);
static EXIT_REQUESTED: AtomicBool = AtomicBool::new(false);

unsafe extern "C" fn applet_hook(hook: u32, _param: *mut c_void) {
    match hook {
        0 => {
            if unsafe { appletGetFocusState() } == 1 {
                FOCUS_GAINED.fetch_add(1, Ordering::SeqCst);
            } else {
                FOCUS_LOST.fetch_add(1, Ordering::SeqCst);
            }
        }
        1 => {
            MODE_CHANGES.fetch_add(1, Ordering::SeqCst);
        }
        3 => EXIT_REQUESTED.store(true, Ordering::SeqCst),
        4 => {
            RESUMED.fetch_add(1, Ordering::SeqCst);
        }
        _ => {}
    }
}

pub fn install_applet_hook() {
    let cookie = Box::leak(Box::new(AppletHookCookie {
        next: core::ptr::null_mut(),
        callback: None,
        param: core::ptr::null_mut(),
    }));
    unsafe { appletHook(cookie, applet_hook, core::ptr::null_mut()) };
}

pub fn in_focus() -> bool {
    unsafe { appletGetFocusState() == 1 }
}

pub fn exit_requested() -> bool {
    EXIT_REQUESTED.load(Ordering::SeqCst)
}

/// One pass of libnx's applet loop. False once HOME has asked the app to close.
pub fn applet_loop() -> bool {
    if exit_requested() {
        return false;
    }
    if !unsafe { appletMainLoop() } {
        EXIT_REQUESTED.store(true, Ordering::SeqCst);
        return false;
    }
    true
}

// ---- Raw pad for the tests that run before Bevy.

pub struct Pad(PadState);

impl Pad {
    pub fn new() -> Pad {
        let mut pad = Pad(PadState::default());
        unsafe {
            padConfigureInput(1, STYLE_SET_STANDARD);
            padInitializeWithMask(&mut pad.0, 1 | (1u64 << 32));
        }
        pad
    }

    pub fn update(&mut self) {
        unsafe { padUpdate(&mut self.0) };
    }

    pub fn held(&self) -> u64 {
        self.0.buttons_cur
    }

    pub fn down(&self) -> u64 {
        self.0.buttons_cur & !self.0.buttons_old
    }

    pub fn style_set(&self) -> u32 {
        self.0.style_set
    }
}

// ---- Horizon's FS service directly, beneath libnx's fsdev and newlib's readdir.

/// libnx `FsDir`: a `Service` (`sf/service.h`), 16 bytes.
#[repr(C)]
#[derive(Default)]
struct FsDir {
    session: u32,
    own_handle: u32,
    object_id: u32,
    pointer_buffer_size: u16,
}

/// libnx `FsDirectoryEntry` (`services/fs.h`), 0x310 bytes.
#[repr(C)]
struct FsDirectoryEntry {
    name: [u8; 0x301],
    pad: [u8; 3],
    kind: i8,
    pad2: [u8; 3],
    file_size: i64,
}

const FS_DIR_OPEN_DIRS_AND_FILES: u32 = (1 << 0) | (1 << 1);

unsafe extern "C" {
    fn fsdevGetDeviceFileSystem(name: *const c_char) -> *mut c_void;
    pub fn fsdevGetLastResult() -> u32;
    fn fsFsOpenDirectory(fs: *mut c_void, path: *const c_char, mode: u32, out: *mut FsDir) -> u32;
    fn fsDirGetEntryCount(dir: *mut FsDir, count: *mut i64) -> u32;
    fn fsDirRead(dir: *mut FsDir, total: *mut i64, max: usize, buf: *mut FsDirectoryEntry) -> u32;
    fn fsDirClose(dir: *mut FsDir);
}

/// An SD card folder as the FS service lists it.
pub struct RawListing {
    /// What `fsDirGetEntryCount` says, or its error.
    pub count: Result<i64, u32>,
    /// Each entry's name bytes, and whether it is a folder.
    pub entries: Vec<(Vec<u8>, bool)>,
    /// Why the listing stopped early, if it did.
    pub error: Option<String>,
}

/// Lists `path` (from the SD card's root, e.g. `/switch/captest_tmp`) with `fsFsOpenDirectory`
/// and `fsDirRead`, the calls libnx's fsdev makes for `readdir`, without fsdev or newlib.
pub fn fs_list_raw(path: &str) -> RawListing {
    let mut listing = RawListing {
        count: Err(0),
        entries: Vec::new(),
        error: None,
    };
    let fs = unsafe { fsdevGetDeviceFileSystem(c"sdmc".as_ptr()) };
    let Ok(cpath) = std::ffi::CString::new(path) else {
        listing.error = Some("the path has a NUL".into());
        return listing;
    };
    if fs.is_null() {
        listing.error = Some("fsdev has no sdmc device".into());
        return listing;
    }
    let mut dir = FsDir::default();
    let rc = unsafe { fsFsOpenDirectory(fs, cpath.as_ptr(), FS_DIR_OPEN_DIRS_AND_FILES, &mut dir) };
    if rc != 0 {
        listing.error = Some(format!("fsFsOpenDirectory: 0x{rc:x}"));
        return listing;
    }
    let mut count = 0i64;
    let rc = unsafe { fsDirGetEntryCount(&mut dir, &mut count) };
    listing.count = if rc == 0 { Ok(count) } else { Err(rc) };
    let mut buf: Vec<FsDirectoryEntry> = (0..8)
        .map(|_| FsDirectoryEntry {
            name: [0; 0x301],
            pad: [0; 3],
            kind: 0,
            pad2: [0; 3],
            file_size: 0,
        })
        .collect();
    loop {
        let mut read = 0i64;
        let rc = unsafe { fsDirRead(&mut dir, &mut read, buf.len(), buf.as_mut_ptr()) };
        if rc != 0 {
            listing.error = Some(format!(
                "fsDirRead after {} entries: 0x{rc:x}",
                listing.entries.len()
            ));
            break;
        }
        if read <= 0 {
            break;
        }
        for entry in &buf[..(read as usize).min(buf.len())] {
            let len = entry.name.iter().position(|&b| b == 0).unwrap_or(entry.name.len());
            listing.entries.push((entry.name[..len].to_vec(), entry.kind == 0));
        }
    }
    unsafe { fsDirClose(&mut dir) };
    listing
}
