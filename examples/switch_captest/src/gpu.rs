//! EGL and OpenGL ES 3 through devkitPro's Mesa (nouveau): the stack wgpu's GL backend, and so
//! Bevy's renderer, would sit on. Checks the context, draws to the screen, reads pixels back, and
//! tries the formats and features Bevy's renderer asks for: HDR and depth targets, MSAA, BC
//! (DXT) textures, compute. The console holds the default window, so main closes it first.

use std::ffi::{CStr, c_char, c_void};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use crate::interactive::{ask, record_answer};
use crate::nx::{self, Pad};
use crate::report::{self, check, check_warn, fail, info, pass, skip, warn};

type EglDisplay = *mut c_void;
type EglConfig = *mut c_void;
type EglSurface = *mut c_void;
type EglContext = *mut c_void;

const EGL_SUCCESS: i32 = 0x3000;
const EGL_ALPHA_SIZE: i32 = 0x3021;
const EGL_BLUE_SIZE: i32 = 0x3022;
const EGL_GREEN_SIZE: i32 = 0x3023;
const EGL_RED_SIZE: i32 = 0x3024;
const EGL_DEPTH_SIZE: i32 = 0x3025;
const EGL_STENCIL_SIZE: i32 = 0x3026;
const EGL_SAMPLES: i32 = 0x3031;
const EGL_SURFACE_TYPE: i32 = 0x3033;
const EGL_NONE: i32 = 0x3038;
const EGL_RENDERABLE_TYPE: i32 = 0x3040;
const EGL_VENDOR: i32 = 0x3053;
const EGL_VERSION: i32 = 0x3054;
const EGL_EXTENSIONS: i32 = 0x3055;
const EGL_HEIGHT: i32 = 0x3056;
const EGL_WIDTH: i32 = 0x3057;
const EGL_CLIENT_APIS: i32 = 0x308D;
const EGL_CONTEXT_MAJOR_VERSION: i32 = 0x3098;
const EGL_CONTEXT_MINOR_VERSION: i32 = 0x30FB;
const EGL_WINDOW_BIT: i32 = 0x0004;
const EGL_OPENGL_ES3_BIT: i32 = 0x0040;
const EGL_OPENGL_ES_API: u32 = 0x30A0;

const GL_NO_ERROR: u32 = 0;
const GL_TRIANGLES: u32 = 0x0004;
const GL_SRC_ALPHA: u32 = 0x0302;
const GL_ONE_MINUS_SRC_ALPHA: u32 = 0x0303;
const GL_BLEND: u32 = 0x0BE2;
const GL_TEXTURE_2D: u32 = 0x0DE1;
const GL_UNSIGNED_BYTE: u32 = 0x1401;
const GL_RGBA: u32 = 0x1908;
const GL_VENDOR: u32 = 0x1F00;
const GL_RENDERER: u32 = 0x1F01;
const GL_VERSION: u32 = 0x1F02;
const GL_EXTENSIONS: u32 = 0x1F03;
const GL_NEAREST: i32 = 0x2600;
const GL_TEXTURE_MAG_FILTER: u32 = 0x2800;
const GL_TEXTURE_MIN_FILTER: u32 = 0x2801;
const GL_COLOR_BUFFER_BIT: u32 = 0x4000;
const GL_DEPTH_BUFFER_BIT: u32 = 0x0100;
const GL_RGBA8: u32 = 0x8058;
const GL_COMPRESSED_RGBA_S3TC_DXT1_EXT: u32 = 0x83F1;
const GL_COMPRESSED_RGBA_S3TC_DXT5_EXT: u32 = 0x83F3;
const GL_TEXTURE0: u32 = 0x84C0;
const GL_RGBA16F: u32 = 0x881A;
const GL_DEPTH24_STENCIL8: u32 = 0x88F0;
const GL_DYNAMIC_READ: u32 = 0x88E9;
const GL_FRAGMENT_SHADER: u32 = 0x8B30;
const GL_VERTEX_SHADER: u32 = 0x8B31;
const GL_COMPILE_STATUS: u32 = 0x8B81;
const GL_LINK_STATUS: u32 = 0x8B82;
const GL_INFO_LOG_LENGTH: u32 = 0x8B84;
const GL_SHADING_LANGUAGE_VERSION: u32 = 0x8B8C;
const GL_R11F_G11F_B10F: u32 = 0x8C3A;
const GL_DEPTH_COMPONENT32F: u32 = 0x8CAC;
const GL_FRAMEBUFFER_COMPLETE: u32 = 0x8CD5;
const GL_COLOR_ATTACHMENT0: u32 = 0x8CE0;
const GL_DEPTH_ATTACHMENT: u32 = 0x8D00;
const GL_FRAMEBUFFER: u32 = 0x8D40;
const GL_RENDERBUFFER: u32 = 0x8D41;
const GL_NUM_EXTENSIONS: u32 = 0x821D;
const GL_DEPTH_STENCIL_ATTACHMENT: u32 = 0x821A;
const GL_SHADER_STORAGE_BUFFER: u32 = 0x90D2;
const GL_COMPUTE_SHADER: u32 = 0x91B9;
const GL_MAP_READ_BIT: u32 = 0x0001;
const GL_SHADER_STORAGE_BARRIER_BIT: u32 = 0x2000;
const GL_BUFFER_UPDATE_BARRIER_BIT: u32 = 0x0200;
const GL_MAX_SAMPLES: u32 = 0x8D57;

unsafe extern "C" {
    fn nwindowGetDefault() -> *mut c_void;
    fn nwindowGetDimensions(win: *mut c_void, width: *mut u32, height: *mut u32) -> i32;
    fn nvExit();

    fn eglGetDisplay(display: *mut c_void) -> EglDisplay;
    fn eglInitialize(dpy: EglDisplay, major: *mut i32, minor: *mut i32) -> u32;
    fn eglBindAPI(api: u32) -> u32;
    fn eglChooseConfig(
        dpy: EglDisplay,
        attribs: *const i32,
        configs: *mut EglConfig,
        size: i32,
        num: *mut i32,
    ) -> u32;
    fn eglGetConfigAttrib(
        dpy: EglDisplay,
        config: EglConfig,
        attribute: i32,
        value: *mut i32,
    ) -> u32;
    fn eglCreateWindowSurface(
        dpy: EglDisplay,
        config: EglConfig,
        win: *mut c_void,
        attribs: *const i32,
    ) -> EglSurface;
    fn eglCreateContext(
        dpy: EglDisplay,
        config: EglConfig,
        share: EglContext,
        attribs: *const i32,
    ) -> EglContext;
    fn eglMakeCurrent(dpy: EglDisplay, draw: EglSurface, read: EglSurface, ctx: EglContext) -> u32;
    fn eglSwapInterval(dpy: EglDisplay, interval: i32) -> u32;
    fn eglSwapBuffers(dpy: EglDisplay, surface: EglSurface) -> u32;
    fn eglQuerySurface(
        dpy: EglDisplay,
        surface: EglSurface,
        attribute: i32,
        value: *mut i32,
    ) -> u32;
    fn eglQueryString(dpy: EglDisplay, name: i32) -> *const c_char;
    fn eglGetProcAddress(name: *const c_char) -> *const c_void;
    fn eglGetError() -> i32;
    fn eglDestroyContext(dpy: EglDisplay, ctx: EglContext) -> u32;
    fn eglDestroySurface(dpy: EglDisplay, surface: EglSurface) -> u32;
    fn eglTerminate(dpy: EglDisplay) -> u32;
    fn eglReleaseThread() -> u32;

    fn glGetString(name: u32) -> *const u8;
    fn glGetStringi(name: u32, index: u32) -> *const u8;
    fn glGetIntegerv(name: u32, out: *mut i32);
    fn glGetIntegeri_v(name: u32, index: u32, out: *mut i32);
    fn glGetError() -> u32;
    fn glViewport(x: i32, y: i32, w: i32, h: i32);
    fn glClearColor(r: f32, g: f32, b: f32, a: f32);
    fn glClear(mask: u32);
    fn glEnable(cap: u32);
    fn glDisable(cap: u32);
    fn glBlendFunc(src: u32, dst: u32);
    fn glFinish();
    fn glReadPixels(x: i32, y: i32, w: i32, h: i32, format: u32, kind: u32, data: *mut c_void);

    fn glCreateShader(kind: u32) -> u32;
    fn glShaderSource(shader: u32, count: i32, strings: *const *const c_char, lengths: *const i32);
    fn glCompileShader(shader: u32);
    fn glGetShaderiv(shader: u32, name: u32, out: *mut i32);
    fn glGetShaderInfoLog(shader: u32, max: i32, len: *mut i32, log: *mut c_char);
    fn glDeleteShader(shader: u32);
    fn glCreateProgram() -> u32;
    fn glAttachShader(program: u32, shader: u32);
    fn glLinkProgram(program: u32);
    fn glGetProgramiv(program: u32, name: u32, out: *mut i32);
    fn glGetProgramInfoLog(program: u32, max: i32, len: *mut i32, log: *mut c_char);
    fn glUseProgram(program: u32);
    fn glDeleteProgram(program: u32);
    fn glGetUniformLocation(program: u32, name: *const c_char) -> i32;
    fn glUniform1f(location: i32, v: f32);
    fn glUniform1i(location: i32, v: i32);
    fn glUniform4f(location: i32, x: f32, y: f32, z: f32, w: f32);

    fn glGenVertexArrays(n: i32, out: *mut u32);
    fn glBindVertexArray(vao: u32);
    fn glDeleteVertexArrays(n: i32, vaos: *const u32);
    fn glDrawArrays(mode: u32, first: i32, count: i32);

    fn glGenTextures(n: i32, out: *mut u32);
    fn glBindTexture(target: u32, texture: u32);
    fn glDeleteTextures(n: i32, textures: *const u32);
    fn glActiveTexture(unit: u32);
    fn glTexParameteri(target: u32, name: u32, value: i32);
    fn glTexStorage2D(target: u32, levels: i32, format: u32, w: i32, h: i32);
    fn glCompressedTexImage2D(
        target: u32,
        level: i32,
        format: u32,
        w: i32,
        h: i32,
        border: i32,
        size: i32,
        data: *const c_void,
    );

    fn glGenFramebuffers(n: i32, out: *mut u32);
    fn glBindFramebuffer(target: u32, framebuffer: u32);
    fn glDeleteFramebuffers(n: i32, framebuffers: *const u32);
    fn glFramebufferTexture2D(
        target: u32,
        attachment: u32,
        textarget: u32,
        texture: u32,
        level: i32,
    );
    fn glFramebufferRenderbuffer(target: u32, attachment: u32, rbtarget: u32, renderbuffer: u32);
    fn glCheckFramebufferStatus(target: u32) -> u32;
    fn glGenRenderbuffers(n: i32, out: *mut u32);
    fn glBindRenderbuffer(target: u32, renderbuffer: u32);
    fn glRenderbufferStorageMultisample(target: u32, samples: i32, format: u32, w: i32, h: i32);
    fn glDeleteRenderbuffers(n: i32, renderbuffers: *const u32);

    fn glGenBuffers(n: i32, out: *mut u32);
    fn glBindBuffer(target: u32, buffer: u32);
    fn glBufferData(target: u32, size: isize, data: *const c_void, usage: u32);
    fn glBindBufferBase(target: u32, index: u32, buffer: u32);
    fn glDeleteBuffers(n: i32, buffers: *const u32);
    fn glDispatchCompute(x: u32, y: u32, z: u32);
    fn glMemoryBarrier(bits: u32);
    fn glMapBufferRange(target: u32, offset: isize, length: isize, access: u32) -> *mut c_void;
    fn glUnmapBuffer(target: u32) -> u8;
}

const TRIANGLE_VS: &CStr = c"#version 300 es
uniform float u_time;
out vec3 v_color;
const vec2 P[3] = vec2[3](vec2(-0.6, -0.6), vec2(0.6, -0.6), vec2(0.0, 0.7));
const vec3 C[3] = vec3[3](vec3(1.0, 0.0, 0.0), vec3(0.0, 1.0, 0.0), vec3(0.0, 0.0, 1.0));
void main() {
    vec2 q = P[gl_VertexID];
    float s = sin(u_time);
    float c = cos(u_time);
    gl_Position = vec4((q.x * c - q.y * s) * 0.5625, q.x * s + q.y * c, 0.0, 1.0);
    v_color = C[gl_VertexID];
}
";

const TRIANGLE_FS: &CStr = c"#version 300 es
precision mediump float;
uniform vec4 u_solid;
in vec3 v_color;
out vec4 o_color;
void main() {
    o_color = u_solid.a > 0.0 ? u_solid : vec4(v_color, 1.0);
}
";

/// A triangle that covers the whole target, with uv 0..1 across it.
const FULLSCREEN_VS: &CStr = c"#version 300 es
out vec2 v_uv;
void main() {
    vec2 p = vec2(float((gl_VertexID << 1) & 2), float(gl_VertexID & 2));
    v_uv = p;
    gl_Position = vec4(p * 2.0 - 1.0, 0.0, 1.0);
}
";

const TEXTURE_FS: &CStr = c"#version 300 es
precision mediump float;
uniform sampler2D u_tex;
in vec2 v_uv;
out vec4 o_color;
void main() {
    o_color = texture(u_tex, v_uv);
}
";

const COLOR_FS: &CStr = c"#version 300 es
precision mediump float;
uniform vec4 u_color;
out vec4 o_color;
void main() {
    o_color = u_color;
}
";

const COMPUTE_CS: &CStr = c"#version 310 es
layout(local_size_x = 64) in;
layout(std430, binding = 0) buffer Data { uint v[]; };
void main() {
    uint i = gl_GlobalInvocationID.x;
    v[i] = i * i + 1u;
}
";

struct Egl {
    display: EglDisplay,
    surface: EglSurface,
    context: EglContext,
    width: i32,
    height: i32,
    /// e.g. 32 for ES 3.2.
    version: u32,
}

static DREW: AtomicBool = AtomicBool::new(false);

fn egl_error() -> String {
    format!("{:#x}", unsafe { eglGetError() })
}

fn gl_error(what: &str) -> Option<String> {
    let mut first = None;
    // glGetError returns one flag per call; drain them all.
    for _ in 0..8 {
        let e = unsafe { glGetError() };
        if e == GL_NO_ERROR {
            break;
        }
        first.get_or_insert(format!("{what}: GL error {e:#x}"));
    }
    first
}

fn cstr(p: *const c_char) -> String {
    if p.is_null() {
        "(null)".into()
    } else {
        unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned()
    }
}

fn gl_string(name: u32) -> String {
    cstr(unsafe { glGetString(name) }.cast())
}

fn get_int(name: u32) -> i32 {
    let mut v = [0i32; 4];
    unsafe { glGetIntegerv(name, v.as_mut_ptr()) };
    v[0]
}

/// Runs the GPU checks. The console must be closed (main reopens it afterwards).
pub fn gpu() {
    report::heading("GPU: EGL + OpenGL ES (Mesa nouveau)");
    if !report::begin("gpu-egl", 30) {
        return;
    }
    let egl = init();
    report::end();
    let Some(egl) = egl else {
        return;
    };

    report::section("gpu-info", 15, || describe(&egl));
    report::section("gpu-proc-address", 10, proc_addresses);
    if report::begin("gpu-draw", 60) {
        draw(&egl);
        report::end();
    }
    report::section("gpu-targets", 30, targets);
    report::section("gpu-bc-textures", 30, bc_textures);
    if egl.version >= 31 {
        report::section("gpu-compute", 30, compute);
    } else {
        skip(
            "gpu-compute",
            format!(
                "the context is ES {}.{}; compute needs 3.1",
                egl.version / 10,
                egl.version % 10
            ),
        );
    }
    report::section("gpu-fill-rate", 30, || fill_rate(&egl));

    if report::begin("gpu-teardown", 15) {
        teardown(egl);
        report::end();
    }
}

/// After main has reopened the console: did the triangle show up on screen?
pub fn ask_seen() {
    if !DREW.load(Ordering::SeqCst) || report::skipped("gpu-seen") {
        return;
    }
    let mut pad = Pad::new();
    let answer = ask(
        &mut pad,
        "Did you just see a spinning red/green/blue triangle on a dark blue screen?",
        30,
    );
    record_answer("gpu-seen", answer, "triangle visible on screen");
}

fn init() -> Option<Egl> {
    unsafe {
        let display = eglGetDisplay(core::ptr::null_mut());
        if display.is_null() {
            fail(
                "gpu-egl",
                format!(
                    "eglGetDisplay(EGL_DEFAULT_DISPLAY) returned no display ({})",
                    egl_error()
                ),
            );
            return None;
        }
        let (mut major, mut minor) = (0, 0);
        let t = Instant::now();
        if eglInitialize(display, &mut major, &mut minor) == 0 {
            fail("gpu-egl", format!("eglInitialize failed ({})", egl_error()));
            return None;
        }
        info(
            "gpu-egl-version",
            format!(
                "EGL {major}.{minor} from {} in {:.0} ms; APIs: {}",
                cstr(eglQueryString(display, EGL_VENDOR)),
                t.elapsed().as_secs_f64() * 1000.0,
                cstr(eglQueryString(display, EGL_CLIENT_APIS))
            ),
        );
        let _ = eglQueryString(display, EGL_VERSION);
        let ext = cstr(eglQueryString(display, EGL_EXTENSIONS));
        let wanted = [
            "EGL_KHR_create_context",
            "EGL_KHR_surfaceless_context",
            "EGL_KHR_gl_colorspace",
            "EGL_KHR_fence_sync",
            "EGL_EXT_pixel_format_float",
        ];
        let have: Vec<&str> = wanted
            .iter()
            .copied()
            .filter(|w| ext.split(' ').any(|e| e == *w))
            .collect();
        let missing: Vec<&str> = wanted
            .iter()
            .copied()
            .filter(|w| !have.contains(w))
            .collect();
        info(
            "gpu-egl-extensions",
            format!(
                "{} EGL extensions; has {have:?}; lacks {missing:?}",
                ext.split(' ').filter(|e| !e.is_empty()).count()
            ),
        );

        if eglBindAPI(EGL_OPENGL_ES_API) == 0 {
            fail(
                "gpu-egl",
                format!("eglBindAPI(OpenGL ES) failed ({})", egl_error()),
            );
            eglTerminate(display);
            return None;
        }
        let attribs = [
            EGL_RENDERABLE_TYPE,
            EGL_OPENGL_ES3_BIT,
            EGL_SURFACE_TYPE,
            EGL_WINDOW_BIT,
            EGL_RED_SIZE,
            8,
            EGL_GREEN_SIZE,
            8,
            EGL_BLUE_SIZE,
            8,
            EGL_ALPHA_SIZE,
            8,
            EGL_DEPTH_SIZE,
            24,
            EGL_STENCIL_SIZE,
            8,
            EGL_NONE,
        ];
        let mut config: EglConfig = core::ptr::null_mut();
        let mut count = 0;
        if eglChooseConfig(display, attribs.as_ptr(), &mut config, 1, &mut count) == 0 || count == 0
        {
            fail(
                "gpu-egl",
                format!("no EGL config with RGBA8, D24S8 and ES 3 ({})", egl_error()),
            );
            eglTerminate(display);
            return None;
        }
        let attr = |a: i32| {
            let mut v = 0;
            eglGetConfigAttrib(display, config, a, &mut v);
            v
        };
        info(
            "gpu-egl-config",
            format!(
                "config: R{}G{}B{}A{} depth {} stencil {} samples {}",
                attr(EGL_RED_SIZE),
                attr(EGL_GREEN_SIZE),
                attr(EGL_BLUE_SIZE),
                attr(EGL_ALPHA_SIZE),
                attr(EGL_DEPTH_SIZE),
                attr(EGL_STENCIL_SIZE),
                attr(EGL_SAMPLES)
            ),
        );

        let surface =
            eglCreateWindowSurface(display, config, nwindowGetDefault(), core::ptr::null());
        if surface.is_null() {
            fail(
                "gpu-egl",
                format!(
                    "eglCreateWindowSurface(nwindowGetDefault()) failed ({})",
                    egl_error()
                ),
            );
            eglTerminate(display);
            return None;
        }
        let mut context: EglContext = core::ptr::null_mut();
        let mut version = 0;
        for (maj, min) in [(3, 2), (3, 1), (3, 0)] {
            let ctx_attribs = [
                EGL_CONTEXT_MAJOR_VERSION,
                maj,
                EGL_CONTEXT_MINOR_VERSION,
                min,
                EGL_NONE,
            ];
            context =
                eglCreateContext(display, config, core::ptr::null_mut(), ctx_attribs.as_ptr());
            if !context.is_null() {
                version = (maj * 10 + min) as u32;
                break;
            }
        }
        if context.is_null() {
            fail(
                "gpu-egl",
                format!(
                    "eglCreateContext failed for ES 3.2, 3.1 and 3.0 ({})",
                    egl_error()
                ),
            );
            eglDestroySurface(display, surface);
            eglTerminate(display);
            return None;
        }
        if eglMakeCurrent(display, surface, surface, context) == 0 {
            fail(
                "gpu-egl",
                format!("eglMakeCurrent failed ({})", egl_error()),
            );
            eglDestroyContext(display, context);
            eglDestroySurface(display, surface);
            eglTerminate(display);
            return None;
        }
        let vsync = eglSwapInterval(display, 1) != 0;
        let (mut width, mut height) = (0, 0);
        eglQuerySurface(display, surface, EGL_WIDTH, &mut width);
        eglQuerySurface(display, surface, EGL_HEIGHT, &mut height);
        let egl_w = width;
        let egl_h = height;
        if width <= 0 || height <= 0 {
            let mut nw_w = 0u32;
            let mut nw_h = 0u32;
            let rc = nwindowGetDimensions(nwindowGetDefault(), &mut nw_w, &mut nw_h);
            if rc == 0 && nw_w > 0 && nw_h > 0 {
                width = nw_w as i32;
                height = nw_h as i32;
            } else {
                width = 1280;
                height = 720;
            }
        }
        pass(
            "gpu-egl",
            format!(
                "ES {}.{} context on a {width}x{height} window surface; swap interval 1: {}",
                version / 10,
                version % 10,
                if vsync { "ok" } else { "refused" }
            ),
        );
        check_warn(
            "gpu-surface-size",
            width == 1280 && height == 720,
            if egl_w <= 0 || egl_h <= 0 {
                format!(
                    "eglQuerySurface returned {egl_w}x{egl_h} (switch-mesa libEGL bug); fell back to nwindow {width}x{height}"
                )
            } else {
                format!("surface is {width}x{height} (handheld: 1280x720)")
            },
        );
        Some(Egl {
            display,
            surface,
            context,
            width,
            height,
            version,
        })
    }
}

fn describe(egl: &Egl) {
    let version = gl_string(GL_VERSION);
    info(
        "gpu-strings",
        format!(
            "{} | {} | {} | GLSL {}",
            gl_string(GL_VENDOR),
            gl_string(GL_RENDERER),
            version,
            gl_string(GL_SHADING_LANGUAGE_VERSION)
        ),
    );
    let count = get_int(GL_NUM_EXTENSIONS).max(0) as u32;
    let mut extensions = Vec::with_capacity(count as usize);
    for i in 0..count {
        extensions.push(cstr(unsafe { glGetStringi(GL_EXTENSIONS, i) }.cast()));
    }
    // What wgpu's GL backend looks for, and what decides which texture formats Bevy can use.
    let wanted = [
        "GL_EXT_texture_compression_s3tc",
        "GL_EXT_texture_compression_rgtc",
        "GL_EXT_texture_compression_bptc",
        "GL_KHR_texture_compression_astc_ldr",
        "GL_EXT_color_buffer_float",
        "GL_EXT_color_buffer_half_float",
        "GL_OES_texture_float_linear",
        "GL_EXT_texture_filter_anisotropic",
        "GL_EXT_disjoint_timer_query",
        "GL_EXT_buffer_storage",
        "GL_EXT_clip_control",
        "GL_EXT_depth_clamp",
        "GL_EXT_base_instance",
        "GL_EXT_multi_draw_indirect",
        "GL_EXT_texture_format_BGRA8888",
        "GL_EXT_sRGB_write_control",
        "GL_KHR_debug",
        "GL_OVR_multiview2",
    ];
    let has = |name: &str| extensions.iter().any(|e| e == name);
    let present: Vec<&str> = wanted.iter().copied().filter(|w| has(w)).collect();
    let absent: Vec<&str> = wanted.iter().copied().filter(|w| !has(w)).collect();
    info(
        "gpu-extensions",
        format!("{count} extensions; has {}", present.join(" ")),
    );
    info(
        "gpu-extensions-missing",
        if absent.is_empty() {
            "none of the ones listed are missing".to_string()
        } else {
            absent.join(" ")
        },
    );
    check_warn(
        "gpu-bc-extension",
        has("GL_EXT_texture_compression_s3tc"),
        "GL_EXT_texture_compression_s3tc (DXT/BC1-3: IW4's textures; wgpu's BC feature also needs rgtc and bptc)",
    );
    check(
        "gpu-es-version",
        egl.version >= 31,
        format!(
            "{version}: wgpu's GL backend needs ES 3.0; compute shaders (Bevy's GPU culling, some post effects) need 3.1"
        ),
    );

    let limits: [(&str, u32); 14] = [
        ("max_texture_size", 0x0D33),
        ("max_3d_texture_size", 0x8073),
        ("max_array_texture_layers", 0x88FF),
        ("max_samples", GL_MAX_SAMPLES),
        ("max_color_attachments", 0x8CDF),
        ("max_draw_buffers", 0x8824),
        ("max_vertex_attribs", 0x8869),
        ("max_texture_image_units", 0x8872),
        ("max_combined_texture_units", 0x8B4D),
        ("max_uniform_block_size", 0x8A30),
        ("max_uniform_buffer_bindings", 0x8A2F),
        ("max_ssbo_block_size", 0x90DE),
        ("max_compute_invocations", 0x90EB),
        ("max_compute_shared_memory", 0x8262),
    ];
    let mut text = String::new();
    for (name, pname) in limits {
        text.push_str(&format!("{name}={} ", get_int(pname)));
    }
    let mut size = [0i32; 3];
    for (i, s) in size.iter_mut().enumerate() {
        unsafe { glGetIntegeri_v(0x91BF, i as u32, s) };
    }
    text.push_str(&format!("max_compute_work_group_size={size:?}"));
    let _ = gl_error("limits");
    info("gpu-limits", text);
    let samples = get_int(GL_MAX_SAMPLES);
    check_warn(
        "gpu-msaa",
        samples >= 4,
        format!("GL_MAX_SAMPLES = {samples} (Bevy's default Msaa is 4x)"),
    );
}

/// glow (wgpu's GL loader) finds every GL function through eglGetProcAddress.
fn proc_addresses() {
    let names = [
        c"glDrawElementsInstanced",
        c"glDrawArraysInstanced",
        c"glTexStorage3D",
        c"glBlitFramebuffer",
        c"glInvalidateFramebuffer",
        c"glFenceSync",
        c"glClientWaitSync",
        c"glGetProgramBinary",
        c"glBindBufferRange",
        c"glDispatchCompute",
        c"glDrawElementsIndirect",
        c"glCopyBufferSubData",
        c"glBufferStorageEXT",
        c"glDebugMessageCallbackKHR",
        c"glObjectLabelKHR",
    ];
    let missing: Vec<String> = names
        .iter()
        .filter(|n| unsafe { eglGetProcAddress(n.as_ptr()) }.is_null())
        .map(|n| n.to_string_lossy().into_owned())
        .collect();
    check(
        "gpu-proc-address",
        missing
            .iter()
            .all(|m| m.ends_with("EXT") || m.ends_with("KHR")),
        if missing.is_empty() {
            format!(
                "eglGetProcAddress found all {} functions glow would load",
                names.len()
            )
        } else {
            format!("eglGetProcAddress returned NULL for {missing:?}")
        },
    );
}

fn compile(kind: u32, source: &CStr, what: &str) -> Option<u32> {
    unsafe {
        let shader = glCreateShader(kind);
        let ptr = source.as_ptr();
        glShaderSource(shader, 1, &ptr, core::ptr::null());
        glCompileShader(shader);
        let mut ok = 0;
        glGetShaderiv(shader, GL_COMPILE_STATUS, &mut ok);
        if ok == 0 {
            let mut len = 0;
            glGetShaderiv(shader, GL_INFO_LOG_LENGTH, &mut len);
            let mut log = vec![0u8; len.max(1) as usize];
            glGetShaderInfoLog(
                shader,
                log.len() as i32,
                core::ptr::null_mut(),
                log.as_mut_ptr().cast(),
            );
            fail(
                "gpu-shader",
                format!(
                    "{what} didn't compile: {}",
                    String::from_utf8_lossy(&log).trim_end_matches('\0').trim()
                ),
            );
            glDeleteShader(shader);
            return None;
        }
        Some(shader)
    }
}

fn program(stages: &[(u32, &CStr)], what: &str) -> Option<u32> {
    unsafe {
        let program = glCreateProgram();
        let mut shaders = Vec::new();
        for (kind, source) in stages {
            match compile(*kind, source, what) {
                Some(s) => {
                    glAttachShader(program, s);
                    shaders.push(s);
                }
                None => {
                    for s in shaders {
                        glDeleteShader(s);
                    }
                    glDeleteProgram(program);
                    return None;
                }
            }
        }
        glLinkProgram(program);
        for s in shaders {
            glDeleteShader(s);
        }
        let mut ok = 0;
        glGetProgramiv(program, GL_LINK_STATUS, &mut ok);
        if ok == 0 {
            let mut len = 0;
            glGetProgramiv(program, GL_INFO_LOG_LENGTH, &mut len);
            let mut log = vec![0u8; len.max(1) as usize];
            glGetProgramInfoLog(
                program,
                log.len() as i32,
                core::ptr::null_mut(),
                log.as_mut_ptr().cast(),
            );
            fail(
                "gpu-shader",
                format!(
                    "{what} didn't link: {}",
                    String::from_utf8_lossy(&log).trim_end_matches('\0').trim()
                ),
            );
            glDeleteProgram(program);
            return None;
        }
        Some(program)
    }
}

fn read_pixel(x: i32, y: i32) -> [u8; 4] {
    let mut px = [0u8; 4];
    unsafe {
        glReadPixels(
            x,
            y,
            1,
            1,
            GL_RGBA,
            GL_UNSIGNED_BYTE,
            px.as_mut_ptr().cast(),
        )
    };
    px
}

fn close(a: [u8; 4], b: [u8; 3]) -> bool {
    (0..3).all(|i| (a[i] as i32 - b[i] as i32).abs() <= 3)
}

fn draw(egl: &Egl) {
    let t = Instant::now();
    let Some(prog) = program(
        &[
            (GL_VERTEX_SHADER, TRIANGLE_VS),
            (GL_FRAGMENT_SHADER, TRIANGLE_FS),
        ],
        "the triangle shaders",
    ) else {
        return;
    };
    info(
        "gpu-shader-compile",
        format!(
            "compiled and linked a vertex + fragment program in {:.1} ms",
            t.elapsed().as_secs_f64() * 1000.0
        ),
    );
    unsafe {
        let mut vao = 0;
        glGenVertexArrays(1, &mut vao);
        glBindVertexArray(vao);
        glUseProgram(prog);
        let u_time = glGetUniformLocation(prog, c"u_time".as_ptr());
        let u_solid = glGetUniformLocation(prog, c"u_solid".as_ptr());
        glViewport(0, 0, egl.width, egl.height);

        // One frame with known colours, read back before it's shown.
        glClearColor(0.1, 0.2, 0.3, 1.0);
        glClear(GL_COLOR_BUFFER_BIT | GL_DEPTH_BUFFER_BIT);
        glUniform1f(u_time, 0.0);
        glUniform4f(u_solid, 1.0, 0.5, 0.0, 1.0);
        glDrawArrays(GL_TRIANGLES, 0, 3);
        let corner = read_pixel(4, 4);
        let centre = read_pixel(egl.width / 2, egl.height / 2);
        if let Some(e) = gl_error("first frame") {
            fail("gpu-readback", e);
        } else {
            check(
                "gpu-readback",
                close(corner, [26, 51, 77]) && close(centre, [255, 128, 0]),
                format!(
                    "clear colour read back as {corner:?} (want ~[26, 51, 77]), triangle centre as {centre:?} (want ~[255, 128, 0])"
                ),
            );
        }
        eglSwapBuffers(egl.display, egl.surface);

        // Four seconds of spinning triangle at swap interval 1.
        glUniform4f(u_solid, 0.0, 0.0, 0.0, 0.0);
        let start = Instant::now();
        let mut last = Instant::now();
        let mut frames = 0u32;
        let mut slow = 0u32;
        let mut worst = Duration::ZERO;
        let mut swap_failed = false;
        while start.elapsed() < Duration::from_secs(4) {
            if !nx::applet_loop() {
                break;
            }
            if !nx::in_focus() {
                std::thread::sleep(Duration::from_millis(16));
                last = Instant::now();
                continue;
            }
            glClear(GL_COLOR_BUFFER_BIT | GL_DEPTH_BUFFER_BIT);
            glUniform1f(u_time, start.elapsed().as_secs_f32() * 2.0);
            glDrawArrays(GL_TRIANGLES, 0, 3);
            if eglSwapBuffers(egl.display, egl.surface) == 0 {
                swap_failed = true;
                break;
            }
            let dt = last.elapsed();
            last = Instant::now();
            if frames > 0 {
                worst = worst.max(dt);
                if dt > Duration::from_millis(20) {
                    slow += 1;
                }
            }
            frames += 1;
        }
        DREW.store(frames > 0, Ordering::SeqCst);
        let secs = start.elapsed().as_secs_f64();
        let fps = frames as f64 / secs.max(0.001);
        if swap_failed {
            fail(
                "gpu-present",
                format!(
                    "eglSwapBuffers failed after {frames} frames ({})",
                    egl_error()
                ),
            );
        } else {
            check(
                "gpu-present",
                (55.0..=62.0).contains(&fps) && slow <= 3,
                format!(
                    "{frames} frames in {secs:.2} s = {fps:.1} fps at swap interval 1; {slow} over 20 ms, worst {:.1} ms",
                    worst.as_secs_f64() * 1000.0
                ),
            );
        }
        let _ = gl_error("triangle loop").map(|e| warn("gpu-draw-errors", e));
        glBindVertexArray(0);
        glDeleteVertexArrays(1, &vao);
        glUseProgram(0);
        glDeleteProgram(prog);
    }
}

/// Makes a framebuffer with a colour texture of `color` and an optional depth renderbuffer, then
/// clears it and reads the clear colour back. MSAA targets only check completeness.
fn target(id: &str, what: &str, color: u32, depth: Option<(u32, u32)>, samples: i32) -> bool {
    unsafe {
        let (w, h) = (256, 256);
        let mut fbo = 0;
        let mut tex = 0;
        let mut color_rb = 0;
        let mut depth_rb = 0;
        glGenFramebuffers(1, &mut fbo);
        glBindFramebuffer(GL_FRAMEBUFFER, fbo);
        if samples > 1 {
            glGenRenderbuffers(1, &mut color_rb);
            glBindRenderbuffer(GL_RENDERBUFFER, color_rb);
            glRenderbufferStorageMultisample(GL_RENDERBUFFER, samples, color, w, h);
            glFramebufferRenderbuffer(
                GL_FRAMEBUFFER,
                GL_COLOR_ATTACHMENT0,
                GL_RENDERBUFFER,
                color_rb,
            );
        } else {
            glGenTextures(1, &mut tex);
            glBindTexture(GL_TEXTURE_2D, tex);
            glTexStorage2D(GL_TEXTURE_2D, 1, color, w, h);
            glFramebufferTexture2D(GL_FRAMEBUFFER, GL_COLOR_ATTACHMENT0, GL_TEXTURE_2D, tex, 0);
        }
        if let Some((format, attachment)) = depth {
            glGenRenderbuffers(1, &mut depth_rb);
            glBindRenderbuffer(GL_RENDERBUFFER, depth_rb);
            glRenderbufferStorageMultisample(GL_RENDERBUFFER, samples.max(0), format, w, h);
            glFramebufferRenderbuffer(GL_FRAMEBUFFER, attachment, GL_RENDERBUFFER, depth_rb);
        }
        let status = glCheckFramebufferStatus(GL_FRAMEBUFFER);
        let errors = gl_error(what);
        let complete = status == GL_FRAMEBUFFER_COMPLETE && errors.is_none();
        let mut readback = String::new();
        let mut ok = complete;
        if complete {
            glViewport(0, 0, w, h);
            glClearColor(0.25, 0.5, 0.75, 1.0);
            glClear(
                GL_COLOR_BUFFER_BIT
                    | if depth.is_some() {
                        GL_DEPTH_BUFFER_BIT
                    } else {
                        0
                    },
            );
            if samples <= 1 && color == GL_RGBA8 {
                let px = read_pixel(10, 10);
                ok = close(px, [64, 128, 191]);
                readback = format!(", cleared and read back {px:?}");
            } else {
                glFinish();
                ok = gl_error("clear").is_none();
            }
        }
        glBindFramebuffer(GL_FRAMEBUFFER, 0);
        if fbo != 0 {
            glDeleteFramebuffers(1, &fbo);
        }
        if tex != 0 {
            glDeleteTextures(1, &tex);
        }
        if color_rb != 0 {
            glDeleteRenderbuffers(1, &color_rb);
        }
        if depth_rb != 0 {
            glDeleteRenderbuffers(1, &depth_rb);
        }
        let detail = match errors {
            Some(e) => e,
            None if status != GL_FRAMEBUFFER_COMPLETE => format!("framebuffer status {status:#x}"),
            None => format!("complete{readback}"),
        };
        check(id, ok, format!("{what}: {detail}"))
    }
}

fn targets() {
    target(
        "gpu-target-rgba8",
        "RGBA8 + Depth24Stencil8",
        GL_RGBA8,
        Some((GL_DEPTH24_STENCIL8, GL_DEPTH_STENCIL_ATTACHMENT)),
        0,
    );
    target(
        "gpu-target-depth32f",
        "RGBA8 + Depth32Float (Bevy's 3D depth format)",
        GL_RGBA8,
        Some((GL_DEPTH_COMPONENT32F, GL_DEPTH_ATTACHMENT)),
        0,
    );
    target(
        "gpu-target-rgba16f",
        "Rgba16Float (Bevy's HDR target)",
        GL_RGBA16F,
        None,
        0,
    );
    target(
        "gpu-target-rg11b10f",
        "Rg11b10Ufloat (Bevy's bloom/HDR option)",
        GL_R11F_G11F_B10F,
        None,
        0,
    );
    target(
        "gpu-target-msaa4",
        "4x MSAA RGBA8 + Depth32Float (Bevy's default Msaa)",
        GL_RGBA8,
        Some((GL_DEPTH_COMPONENT32F, GL_DEPTH_ATTACHMENT)),
        4,
    );
}

/// Uploads one 4x4 BC1 and one BC3 block, then draws each into an RGBA8 target and reads the
/// colour back: proof the GPU decodes DXT itself, so IW4's textures need no CPU decode.
fn bc_textures() {
    // BC1: both endpoints pure red (RGB565 0xF800), every index 0.
    let bc1: [u8; 8] = [0x00, 0xF8, 0x00, 0xF8, 0, 0, 0, 0];
    // BC3: alpha endpoints 255/255, all alpha indices 0; colour endpoints pure green (0x07E0).
    let bc3: [u8; 16] = [
        255, 255, 0, 0, 0, 0, 0, 0, 0xE0, 0x07, 0xE0, 0x07, 0, 0, 0, 0,
    ];
    let Some(prog) = program(
        &[
            (GL_VERTEX_SHADER, FULLSCREEN_VS),
            (GL_FRAGMENT_SHADER, TEXTURE_FS),
        ],
        "the texture shaders",
    ) else {
        return;
    };
    unsafe {
        let mut vao = 0;
        glGenVertexArrays(1, &mut vao);
        glBindVertexArray(vao);
        glUseProgram(prog);
        glUniform1i(glGetUniformLocation(prog, c"u_tex".as_ptr()), 0);
        let mut fbo = 0;
        let mut target_tex = 0;
        glGenTextures(1, &mut target_tex);
        glBindTexture(GL_TEXTURE_2D, target_tex);
        glTexStorage2D(GL_TEXTURE_2D, 1, GL_RGBA8, 64, 64);
        glGenFramebuffers(1, &mut fbo);
        glBindFramebuffer(GL_FRAMEBUFFER, fbo);
        glFramebufferTexture2D(
            GL_FRAMEBUFFER,
            GL_COLOR_ATTACHMENT0,
            GL_TEXTURE_2D,
            target_tex,
            0,
        );
        glViewport(0, 0, 64, 64);

        for (id, name, format, data, want) in [
            (
                "gpu-bc1",
                "BC1 (DXT1)",
                GL_COMPRESSED_RGBA_S3TC_DXT1_EXT,
                &bc1[..],
                [255u8, 0, 0],
            ),
            (
                "gpu-bc3",
                "BC3 (DXT5)",
                GL_COMPRESSED_RGBA_S3TC_DXT5_EXT,
                &bc3[..],
                [0, 255, 0],
            ),
        ] {
            let mut tex = 0;
            glActiveTexture(GL_TEXTURE0);
            glGenTextures(1, &mut tex);
            glBindTexture(GL_TEXTURE_2D, tex);
            glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MIN_FILTER, GL_NEAREST);
            glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MAG_FILTER, GL_NEAREST);
            glCompressedTexImage2D(
                GL_TEXTURE_2D,
                0,
                format,
                4,
                4,
                0,
                data.len() as i32,
                data.as_ptr().cast(),
            );
            if let Some(e) = gl_error("glCompressedTexImage2D") {
                fail(id, format!("{name} upload refused: {e}"));
            } else {
                glClearColor(0.0, 0.0, 0.0, 1.0);
                glClear(GL_COLOR_BUFFER_BIT);
                glDrawArrays(GL_TRIANGLES, 0, 3);
                let px = read_pixel(32, 32);
                check(
                    id,
                    close(px, want),
                    format!("{name} texture sampled as {px:?} (want ~{want:?})"),
                );
            }
            glBindTexture(GL_TEXTURE_2D, 0);
            glDeleteTextures(1, &tex);
        }
        glBindFramebuffer(GL_FRAMEBUFFER, 0);
        glDeleteFramebuffers(1, &fbo);
        glDeleteTextures(1, &target_tex);
        glBindVertexArray(0);
        glDeleteVertexArrays(1, &vao);
        glUseProgram(0);
        glDeleteProgram(prog);
    }
}

fn compute() {
    const N: usize = 4096;
    let t = Instant::now();
    let Some(prog) = program(&[(GL_COMPUTE_SHADER, COMPUTE_CS)], "the compute shader") else {
        return;
    };
    let compile_ms = t.elapsed().as_secs_f64() * 1000.0;
    unsafe {
        let mut buf = 0;
        glGenBuffers(1, &mut buf);
        glBindBuffer(GL_SHADER_STORAGE_BUFFER, buf);
        glBufferData(
            GL_SHADER_STORAGE_BUFFER,
            (N * 4) as isize,
            core::ptr::null(),
            GL_DYNAMIC_READ,
        );
        glBindBufferBase(GL_SHADER_STORAGE_BUFFER, 0, buf);
        glUseProgram(prog);
        let t = Instant::now();
        glDispatchCompute((N / 64) as u32, 1, 1);
        glMemoryBarrier(GL_SHADER_STORAGE_BARRIER_BIT | GL_BUFFER_UPDATE_BARRIER_BIT);
        let ptr = glMapBufferRange(
            GL_SHADER_STORAGE_BUFFER,
            0,
            (N * 4) as isize,
            GL_MAP_READ_BIT,
        );
        if ptr.is_null() {
            fail(
                "gpu-compute",
                format!("glMapBufferRange returned NULL ({:?})", gl_error("map")),
            );
        } else {
            let data = std::slice::from_raw_parts(ptr.cast::<u32>(), N);
            let bad = (0..N)
                .filter(|&i| data[i] != (i as u32).wrapping_mul(i as u32).wrapping_add(1))
                .count();
            let ms = t.elapsed().as_secs_f64() * 1000.0;
            glUnmapBuffer(GL_SHADER_STORAGE_BUFFER);
            check(
                "gpu-compute",
                bad == 0,
                format!(
                    "compute shader filled a {N}-entry storage buffer: {bad} wrong values (compile {compile_ms:.1} ms, dispatch + map {ms:.2} ms)"
                ),
            );
        }
        let _ = gl_error("compute").map(|e| warn("gpu-compute-errors", e));
        glBindBuffer(GL_SHADER_STORAGE_BUFFER, 0);
        glDeleteBuffers(1, &buf);
        glUseProgram(0);
        glDeleteProgram(prog);
    }
}

/// Full-screen blended triangles into the window: how many 720p layers fit in a 60 Hz frame.
fn fill_rate(egl: &Egl) {
    let Some(prog) = program(
        &[
            (GL_VERTEX_SHADER, FULLSCREEN_VS),
            (GL_FRAGMENT_SHADER, COLOR_FS),
        ],
        "the fill shaders",
    ) else {
        return;
    };
    unsafe {
        let mut vao = 0;
        glGenVertexArrays(1, &mut vao);
        glBindVertexArray(vao);
        glUseProgram(prog);
        glUniform4f(
            glGetUniformLocation(prog, c"u_color".as_ptr()),
            0.2,
            0.1,
            0.05,
            0.1,
        );
        glViewport(0, 0, egl.width, egl.height);
        glEnable(GL_BLEND);
        glBlendFunc(GL_SRC_ALPHA, GL_ONE_MINUS_SRC_ALPHA);
        let layers = 100;
        let mut best = f64::MAX;
        for _ in 0..5 {
            if !nx::applet_loop() {
                break;
            }
            glClear(GL_COLOR_BUFFER_BIT);
            glFinish();
            let t = Instant::now();
            for _ in 0..layers {
                glDrawArrays(GL_TRIANGLES, 0, 3);
            }
            glFinish();
            best = best.min(t.elapsed().as_secs_f64());
            eglSwapBuffers(egl.display, egl.surface);
        }
        glDisable(GL_BLEND);
        if best < f64::MAX {
            let pixels = (egl.width as f64) * (egl.height as f64) * layers as f64;
            let gpix = pixels / best / 1e9;
            let per_frame = (gpix * 1e9 / 60.0) / ((egl.width as f64) * (egl.height as f64));
            info(
                "gpu-fill-rate",
                format!(
                    "{layers} blended {}x{} layers in {:.2} ms: {gpix:.2} Gpixel/s, about {per_frame:.0} full-screen layers per 60 Hz frame",
                    egl.width,
                    egl.height,
                    best * 1000.0
                ),
            );
        }
        let _ = gl_error("fill").map(|e| warn("gpu-fill-errors", e));
        glBindVertexArray(0);
        glDeleteVertexArrays(1, &vao);
        glUseProgram(0);
        glDeleteProgram(prog);
    }
}

fn teardown(egl: Egl) {
    unsafe {
        let current = eglMakeCurrent(
            egl.display,
            core::ptr::null_mut(),
            core::ptr::null_mut(),
            core::ptr::null_mut(),
        ) != 0;
        let context = eglDestroyContext(egl.display, egl.context) != 0;
        let surface = eglDestroySurface(egl.display, egl.surface) != 0;
        let terminate = eglTerminate(egl.display) != 0;
        let release = eglReleaseThread() != 0;
        nvExit();
        let error = eglGetError();
        check(
            "gpu-teardown",
            current && context && surface && terminate && release,
            format!(
                "release current {current}, destroy context {context}, destroy surface {surface}, terminate {terminate}, release thread {release} (last EGL error {})",
                if error == EGL_SUCCESS {
                    "none".to_string()
                } else {
                    format!("{error:#x}")
                }
            ),
        );
    }
}
