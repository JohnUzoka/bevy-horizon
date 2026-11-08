//! A page of text for a game that can't start: what went wrong and what to do, on the screen and not only in
//! `log.txt`, before the game has a window. [`show`] takes the screen from the splash (`splash::close`), writes the
//! page with libnx's console (80 columns by 45 rows on the default `NWindow`) and waits until the player presses A, B
//! or Plus, or the system asks the app to exit, then gives the screen back (`consoleExit`).
//!
//! libnx's error applet would look like the system's own errors, but it files an error report in the console's system
//! log, and from 10.0.0 it brings the app down when started from the album; libnx's console needs neither. Off the
//! Switch [`show`] returns at once: a game prints the same message to stderr as well.

/// The console's width in characters (libnx's default console: 1280 pixels of 16-pixel glyphs).
#[cfg_attr(not(target_os = "horizon"), allow(dead_code))]
const COLUMNS: usize = 80;

/// Shows `title` and `text` (each line wrapped at word boundaries to the screen's width, blank lines kept) until the
/// player presses A, B or Plus. For a game that can't start: call it before the game has a window, then exit.
pub fn show(title: &str, text: &str) {
    #[cfg(target_os = "horizon")]
    on_console(&page(title, text));
    #[cfg(not(target_os = "horizon"))]
    let _ = (title, text);
}

/// The page: the title in yellow, the text wrapped two columns short of the edge (a line that fills the last column
/// would make the console add a blank one), and the way out.
#[cfg_attr(not(target_os = "horizon"), allow(dead_code))]
fn page(title: &str, text: &str) -> String {
    let width = COLUMNS - 2;
    let mut out = format!("\n \x1b[33;1m{title}\x1b[0m\n\n");
    for line in text.lines() {
        let indent: String = line.chars().take_while(|c| *c == ' ').collect();
        let mut current = String::new();
        for word in line.split_whitespace() {
            if !current.is_empty() && 1 + current.chars().count() + 1 + word.chars().count() > width {
                out.push_str(&format!(" {current}\n"));
                current.clear();
            }
            if current.is_empty() {
                current.push_str(&indent);
            } else {
                current.push(' ');
            }
            current.push_str(word);
        }
        if current.is_empty() {
            out.push('\n');
        } else {
            out.push_str(&format!(" {current}\n"));
        }
    }
    out.push_str("\n \x1b[36mPress A, B or + to return to the HOME menu.\x1b[0m\n");
    out
}

#[cfg(target_os = "horizon")]
mod ffi {
    use core::ffi::c_void;

    /// libnx `PadState` (`switch/runtime/pad.h`): 56 bytes.
    #[repr(C)]
    #[derive(Default)]
    pub struct PadState {
        pub id_mask: u8,
        pub active_id_mask: u8,
        pub read_handheld: bool,
        pub active_handheld: bool,
        pub style_set: u32,
        pub attributes: u32,
        pub buttons_cur: u64,
        pub buttons_old: u64,
        pub sticks: [[i32; 2]; 2],
        pub gc_triggers: [u32; 2],
    }

    /// libnx `HidNpadStyleSet_NpadStandard`: Pro Controller, handheld, and Joy-Con as a pair or one alone.
    pub const NPAD_STANDARD: u32 = 0x1F;
    /// `padInitializeDefault`'s controllers, `HidNpadIdType_No1` and `HidNpadIdType_Handheld` (0x20), as the mask
    /// libnx's inline function builds.
    pub const PLAYER_1_AND_HANDHELD: u64 = 1 | (1 << 0x20);
    /// libnx `HidNpadButton_A`, `_B` and `_Plus`.
    pub const CLOSE_BUTTONS: u64 = 1 | (1 << 1) | (1 << 10);

    unsafe extern "C" {
        /// With a null console, libnx's default one on the default `NWindow`; the first call also makes it newlib's
        /// stdout and stderr device (`devoptab_list[1]` and `[2]`, the device named "con"). fds 1 and 2 keep
        /// pointing at the log through their own `dup2`ed handle, so the page is written to that device directly.
        pub fn consoleInit(console: *mut c_void) -> *mut c_void;
        pub fn consoleUpdate(console: *mut c_void);
        pub fn consoleExit(console: *mut c_void);
        pub fn padConfigureInput(max_players: u32, style_set: u32);
        pub fn padInitializeWithMask(pad: *mut PadState, mask: u64);
        pub fn padUpdate(pad: *mut PadState);
        /// False once the system asks the app to exit.
        pub fn appletMainLoop() -> bool;
    }
}

#[cfg(target_os = "horizon")]
fn on_console(page: &str) {
    use core::ptr::null_mut;
    crate::splash::close();
    // SAFETY: the console is libnx's default one, made and released here on this thread; nothing else draws to the
    // default window now that the splash is down, and the game has no window yet. `pad` outlives every call given it.
    unsafe {
        if ffi::consoleInit(null_mut()).is_null() {
            eprintln!("[Notice] libnx's console didn't start: the message is only in log.txt");
            return;
        }
        if !write_to_console(page) {
            eprintln!("[Notice] newlib's stdout device isn't libnx's console: the message is only in log.txt");
        }
        let mut pad = ffi::PadState::default();
        ffi::padConfigureInput(1, ffi::NPAD_STANDARD);
        ffi::padInitializeWithMask(&mut pad, ffi::PLAYER_1_AND_HANDHELD);
        // A button already down when the page comes up (the A that started the game) counts once it is let go.
        let mut armed = false;
        while ffi::appletMainLoop() {
            ffi::padUpdate(&mut pad);
            let held = pad.buttons_cur & ffi::CLOSE_BUTTONS;
            if armed && held & !pad.buttons_old != 0 {
                break;
            }
            armed |= held == 0;
            ffi::consoleUpdate(null_mut());
        }
        ffi::consoleExit(null_mut());
    }
    eprintln!("[Notice] closed");
}

/// Writes `text` with the `write_r` of newlib's stdout device, once `consoleInit` has made it libnx's console ("con");
/// false (nothing written) when it isn't.
#[cfg(target_os = "horizon")]
unsafe fn write_to_console(text: &str) -> bool {
    use crate::logging::ffi;
    use core::ptr::null_mut;
    // SAFETY: devoptab_list is newlib's device table and index 1 (STD_OUT) is within it; a non-null entry is a
    // devoptab_t of DEVOPTAB_WORDS words whose name is a NUL-terminated string. libnx's console write takes neither
    // its reent nor its file argument.
    unsafe {
        let device = (&raw const ffi::devoptab_list).cast::<*const usize>().add(1).read();
        if device.is_null() {
            return false;
        }
        let name = device.add(ffi::DEVOPTAB_NAME).read() as *const core::ffi::c_char;
        if name.is_null() || core::ffi::CStr::from_ptr(name).to_bytes() != b"con" {
            return false;
        }
        let write = device.add(ffi::DEVOPTAB_WRITE_R).read();
        if write == 0 {
            return false;
        }
        let write = core::mem::transmute::<usize, ffi::WriteR>(write);
        write(null_mut(), null_mut(), text.as_ptr().cast(), text.len());
    }
    true
}
