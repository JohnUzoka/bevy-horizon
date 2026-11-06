//! The Switch's software keyboard: libnx's `swkbd`, the system's keyboard applet, for text a game asks the player
//! for (a name, a password) where a controller has no keys to type with. It takes over the screen until the player
//! confirms or cancels and hands back the whole text, blocking the calling thread meanwhile: the game's frames stop,
//! the render thread finishes the one it has.
//!
//! From the Minecraft port's `horizon-nx::keyboard`, with a header, a password mode and a length limit. Off the
//! Switch `Keyboard::show` returns `None`, so a game falls back to its own key handling.

/// What the keyboard shows and takes.
#[derive(Clone, Copy, Debug, Default)]
pub struct Keyboard<'a> {
    /// The text it starts with, the cursor at its end.
    pub initial: &'a str,
    /// What the text is for ("User name"): the keyboard's header, and its guide text while the box is empty.
    pub header: &'a str,
    /// The most characters the text may have (0: no limit). libnx sets that limit in the keyboard with an inline
    /// function (`swkbdConfigSetStringLenMax`) that can't be linked, so the keyboard takes more and the text handed
    /// back is cut to it.
    pub max_chars: usize,
    /// Dots for what is typed (libnx's password preset).
    pub password: bool,
}

impl Keyboard<'_> {
    /// Shows the keyboard and waits for the player: the text they confirmed (maybe empty), or `None` when they
    /// cancelled or the keyboard couldn't start, and always off the Switch.
    pub fn show(&self) -> Option<String> {
        #[cfg(target_os = "horizon")]
        {
            let text = show(self)?;
            Some(if self.max_chars > 0 { text.chars().take(self.max_chars).collect() } else { text })
        }
        #[cfg(not(target_os = "horizon"))]
        {
            None
        }
    }
}

/// The text the keyboard can hand back: 1 KiB of UTF-8, the NUL included.
#[cfg(target_os = "horizon")]
const TEXT_BYTES: usize = 1024;

/// libnx's `SwkbdConfig` (`switch/applets/swkbd.h`: 1264 bytes, 8-aligned, in the devkitPro libnx of the toolchain
/// image). Only libnx reads or writes its fields (`swkbdCreate` clears and fills them), so it is an opaque buffer with
/// room to spare for a newer libnx.
#[cfg(target_os = "horizon")]
#[repr(C, align(16))]
struct SwkbdConfig([u8; 4096]);

#[cfg(target_os = "horizon")]
unsafe extern "C" {
    fn swkbdCreate(c: *mut SwkbdConfig, max_dictwords: i32) -> u32;
    fn swkbdClose(c: *mut SwkbdConfig);
    fn swkbdConfigMakePresetDefault(c: *mut SwkbdConfig);
    fn swkbdConfigMakePresetPassword(c: *mut SwkbdConfig);
    fn swkbdConfigSetHeaderText(c: *mut SwkbdConfig, text: *const std::ffi::c_char);
    fn swkbdConfigSetGuideText(c: *mut SwkbdConfig, text: *const std::ffi::c_char);
    fn swkbdConfigSetInitialText(c: *mut SwkbdConfig, text: *const std::ffi::c_char);
    fn swkbdShow(c: *mut SwkbdConfig, out: *mut std::ffi::c_char, out_size: usize) -> u32;
}

#[cfg(target_os = "horizon")]
fn show(k: &Keyboard) -> Option<String> {
    use std::ffi::CString;
    let c_text = |s: &str| CString::new(s.replace('\0', "")).unwrap_or_default();
    let (initial, header) = (c_text(k.initial), c_text(k.header));
    let mut config = Box::new(SwkbdConfig([0; 4096]));
    let config: *mut SwkbdConfig = &mut *config;
    let mut out = [0u8; TEXT_BYTES];
    // SAFETY: `config` is zeroed, aligned and larger than libnx's struct, and lives to the end of this function;
    // swkbdCreate initializes it and swkbdClose frees what swkbdCreate allocated. The setters copy their strings into
    // the keyboard's own buffers, and swkbdShow writes at most `out.len()` bytes, the NUL included.
    let rc = unsafe {
        if swkbdCreate(config, 0) != 0 {
            return None;
        }
        if k.password {
            swkbdConfigMakePresetPassword(config);
        } else {
            swkbdConfigMakePresetDefault(config);
        }
        if !k.header.is_empty() {
            swkbdConfigSetHeaderText(config, header.as_ptr());
            swkbdConfigSetGuideText(config, header.as_ptr());
        }
        swkbdConfigSetInitialText(config, initial.as_ptr());
        let rc = swkbdShow(config, out.as_mut_ptr().cast(), out.len());
        swkbdClose(config);
        rc
    };
    if rc != 0 {
        return None;
    }
    let end = out.iter().position(|&b| b == 0).unwrap_or(out.len());
    Some(String::from_utf8_lossy(&out[..end]).into_owned())
}
