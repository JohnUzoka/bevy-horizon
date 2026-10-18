//! Results: one line per check, written to captest_results.txt (synced after every line), log.txt
//! and the screen. `captest_running.txt` names the test in progress: if the app dies, the next run
//! finds it there, adds it to captest_skip.txt and skips it, so one crash can't block the rest.
//! A watchdog thread ends the app if a test or a Bevy frame stops making progress.

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use crate::nx;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Status {
    Pass,
    Fail,
    Warn,
    Info,
    Skip,
    Crash,
}

impl Status {
    pub fn tag(self) -> &'static str {
        match self {
            Status::Pass => "PASS",
            Status::Fail => "FAIL",
            Status::Warn => "WARN",
            Status::Info => "INFO",
            Status::Skip => "SKIP",
            Status::Crash => "CRASH",
        }
    }

    fn color(self) -> u8 {
        match self {
            Status::Pass => 32,
            Status::Fail | Status::Crash => 31,
            Status::Warn => 33,
            Status::Info => 36,
            Status::Skip => 35,
        }
    }
}

struct State {
    file: Option<File>,
    counts: [u32; 6],
    problems: Vec<String>,
    skip: Vec<String>,
}

static STATE: Mutex<Option<State>> = Mutex::new(None);
static DIR: OnceLock<String> = OnceLock::new();
static START: OnceLock<Instant> = OnceLock::new();

fn lock() -> MutexGuard<'static, Option<State>> {
    STATE.lock().unwrap_or_else(|e| e.into_inner())
}

pub fn dir() -> &'static str {
    DIR.get()
        .map(String::as_str)
        .unwrap_or("sdmc:/switch/switch_captest")
}

pub fn path(name: &str) -> String {
    format!("{}/{name}", dir())
}

pub fn elapsed() -> f32 {
    START
        .get()
        .map(|s| s.elapsed().as_secs_f32())
        .unwrap_or(0.0)
}

pub fn init() {
    // bevy_horizon sets BEVY_ASSET_ROOT to the NRO's folder before main runs.
    let dir =
        std::env::var("BEVY_ASSET_ROOT").unwrap_or_else(|_| "sdmc:/switch/switch_captest".into());
    let _ = DIR.set(dir);
    let _ = START.set(Instant::now());

    let marker = fs::read_to_string(path("captest_running.txt"))
        .unwrap_or_default()
        .trim()
        .to_string();
    let mut skip: Vec<String> = fs::read_to_string(path("captest_skip.txt"))
        .unwrap_or_default()
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect();
    if !marker.is_empty() && !skip.contains(&marker) {
        skip.push(marker.clone());
        let _ = fs::write(path("captest_skip.txt"), skip.join("\n") + "\n");
    }

    let results = path("captest_results.txt");
    if fs::metadata(&results).is_ok() {
        let prev = path("captest_results.prev.txt");
        let _ = fs::remove_file(&prev);
        let _ = fs::rename(&results, &prev);
    }
    let file = File::create(&results).ok();
    let opened = file.is_some();
    *lock() = Some(State {
        file,
        counts: [0; 6],
        problems: Vec::new(),
        skip: skip.clone(),
    });
    clear_marker();

    if !opened {
        record(
            Status::Warn,
            "results-file",
            format!("couldn't create {results}; results go to log.txt only"),
        );
    }
    if !marker.is_empty() {
        record(
            Status::Crash,
            &marker,
            "the previous run stopped during this test (a crash, a hang, or the system ended the app), so it's skipped now. Delete captest_skip.txt to retry it",
        );
    }
    for id in skip.iter().filter(|id| **id != marker) {
        record(
            Status::Info,
            "skip-list",
            format!("{id} is in captest_skip.txt from an earlier run; skipping it"),
        );
    }
}

pub fn record(status: Status, id: &str, msg: impl AsRef<str>) {
    let msg = msg.as_ref();
    let line = format!("{:7.1}s {:<5} {id:<28} {msg}\n", elapsed(), status.tag());
    print!("[Captest] {line}");
    let mut guard = lock();
    if let Some(st) = guard.as_mut() {
        st.counts[status as usize] += 1;
        if matches!(status, Status::Fail | Status::Crash | Status::Warn) {
            st.problems.push(format!("{} {id}: {msg}", status.tag()));
        }
        if let Some(f) = st.file.as_mut() {
            let _ = f.write_all(line.as_bytes());
            let _ = f.sync_all();
        }
    }
    let short: String = format!("{:<5} {id}: {msg}", status.tag())
        .chars()
        .take(79)
        .collect();
    nx::console_write(format!("\x1b[{}m{short}\x1b[0m\n", status.color()).as_bytes());
    update_screen(false);
    drop(guard);
}

static LAST_SCREEN_UPDATE: AtomicU64 = AtomicU64::new(0);

/// consoleUpdate waits for the next vsync, so result lines refresh the screen at most 10 times a
/// second; prompts force it. Callers hold the STATE lock.
fn update_screen(force: bool) {
    let now = nx::ticks();
    if force || now.wrapping_sub(LAST_SCREEN_UPDATE.load(Ordering::Relaxed)) > nx::tick_freq() / 10
    {
        LAST_SCREEN_UPDATE.store(now, Ordering::Relaxed);
        nx::console_update();
    }
}

pub fn flush_screen() {
    let _guard = lock();
    update_screen(true);
}

pub fn pass(id: &str, msg: impl AsRef<str>) {
    record(Status::Pass, id, msg);
}
pub fn fail(id: &str, msg: impl AsRef<str>) {
    record(Status::Fail, id, msg);
}
pub fn warn(id: &str, msg: impl AsRef<str>) {
    record(Status::Warn, id, msg);
}
pub fn info(id: &str, msg: impl AsRef<str>) {
    record(Status::Info, id, msg);
}
pub fn skip(id: &str, msg: impl AsRef<str>) {
    record(Status::Skip, id, msg);
}

/// PASS if `ok`, otherwise FAIL. Returns `ok`.
pub fn check(id: &str, ok: bool, msg: impl AsRef<str>) -> bool {
    record(if ok { Status::Pass } else { Status::Fail }, id, msg);
    ok
}

/// PASS if `ok`, otherwise WARN: for behaviour that differs from desktop but has a workaround.
pub fn check_warn(id: &str, ok: bool, msg: impl AsRef<str>) -> bool {
    record(if ok { Status::Pass } else { Status::Warn }, id, msg);
    ok
}

/// Screen only (prompts and live counters), not the results file.
pub fn screen(text: &str) {
    let _guard = lock();
    nx::console_write(text.as_bytes());
    update_screen(true);
}

/// Rewrites the current screen line in place.
pub fn status_line(text: &str) {
    let short: String = text.chars().take(78).collect();
    screen(&format!("\r{short:<78}"));
}

pub fn heading(title: &str) {
    println!("[Captest] ==== {title} ====");
    screen(&format!("\n\x1b[1;37m== {title} ==\x1b[0m\n"));
}

pub fn skipped(id: &str) -> bool {
    lock()
        .as_ref()
        .is_some_and(|st| st.skip.iter().any(|s| s == id))
}

fn set_marker(id: &str) {
    let _ = fs::write(path("captest_running.txt"), id);
}

fn clear_marker() {
    let _ = fs::write(path("captest_running.txt"), "");
}

/// Starts a guarded test: false (and a SKIP line) if an earlier run died in it.
pub fn begin(id: &str, limit_s: u32) -> bool {
    if skipped(id) {
        skip(
            id,
            "skipped: an earlier run crashed or hung here (delete captest_skip.txt to retry)",
        );
        return false;
    }
    set_marker(id);
    arm(id, limit_s);
    true
}

pub fn end() {
    disarm();
    clear_marker();
    flush_screen();
}

pub fn section(id: &str, limit_s: u32, f: impl FnOnce()) {
    if begin(id, limit_s) {
        f();
        end();
    }
}

pub fn counts() -> [u32; 6] {
    lock().as_ref().map(|st| st.counts).unwrap_or_default()
}

pub fn summary() -> String {
    let c = counts();
    format!(
        "{} pass, {} fail, {} warn, {} info, {} skip, {} crash",
        c[0], c[1], c[2], c[3], c[4], c[5]
    )
}

/// Writes the summary and every FAIL/WARN/CRASH again at the end of the results file.
pub fn finish(note: &str) {
    let summary = summary();
    let problems = lock()
        .as_ref()
        .map(|st| st.problems.clone())
        .unwrap_or_default();
    let mut text = format!("\n==== SUMMARY: {summary} ({note}) ====\n");
    for p in &problems {
        text.push_str("  ");
        text.push_str(p);
        text.push('\n');
    }
    print!("{text}");
    if let Some(st) = lock().as_mut()
        && let Some(f) = st.file.as_mut()
    {
        let _ = f.write_all(text.as_bytes());
        let _ = f.sync_all();
    }
}

// ---- Watchdog. Counts its own 500 ms sleeps while the app has focus, so time spent on the HOME
// menu (or suspended) never counts against a test.

static WD_TICKS: AtomicU64 = AtomicU64::new(0);
static WD_DEADLINE: AtomicU64 = AtomicU64::new(0);
static WD_FRAME_DEADLINE: AtomicU64 = AtomicU64::new(0);
static WD_ID: Mutex<String> = Mutex::new(String::new());
static WD_STOP: AtomicBool = AtomicBool::new(false);
static WD_THREAD: Mutex<Option<std::thread::JoinHandle<()>>> = Mutex::new(None);

pub fn arm(id: &str, limit_s: u32) {
    if let Ok(mut s) = WD_ID.lock() {
        s.clear();
        s.push_str(id);
    }
    WD_DEADLINE.store(
        WD_TICKS.load(Ordering::SeqCst) + 2 * limit_s as u64 + 1,
        Ordering::SeqCst,
    );
}

pub fn disarm() {
    WD_DEADLINE.store(0, Ordering::SeqCst);
}

/// Called every Bevy frame: the app is ended if no frame finishes for `limit_s` seconds.
pub fn kick_frame(limit_s: u32) {
    WD_FRAME_DEADLINE.store(
        WD_TICKS.load(Ordering::SeqCst) + 2 * limit_s as u64 + 1,
        Ordering::SeqCst,
    );
}

pub fn stop_frame_watch() {
    WD_FRAME_DEADLINE.store(0, Ordering::SeqCst);
}

pub fn start_watchdog() {
    let (tx, rx) = std::sync::mpsc::channel::<String>();
    let spawned = std::thread::Builder::new()
        .name("captest-watchdog".into())
        .stack_size(256 * 1024)
        .spawn(move || {
            let prio = unsafe { nx::svcSetThreadPriority(nx::CUR_THREAD, 0x2B) };
            let pin = unsafe { nx::svcSetThreadCoreMask(nx::CUR_THREAD, 2, 1 << 2) };
            let ok = |rc: u32| {
                if rc == 0 {
                    "ok".to_string()
                } else {
                    nx::rc_text(rc)
                }
            };
            let _ = tx.send(format!(
                "watchdog running on core {} (priority 0x2B: {}, pin to core 2: {})",
                nx::core_now(),
                ok(prio),
                ok(pin)
            ));
            while !WD_STOP.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(500));
                if WD_STOP.load(Ordering::SeqCst) {
                    break;
                }
                if !nx::in_focus() {
                    continue;
                }
                let now = WD_TICKS.fetch_add(1, Ordering::SeqCst) + 1;
                let test = WD_DEADLINE.load(Ordering::SeqCst);
                let frame = WD_FRAME_DEADLINE.load(Ordering::SeqCst);
                if test != 0 && now >= test {
                    fire("the test ran past its time limit");
                }
                if frame != 0 && now >= frame {
                    fire("no Bevy frame finished for 15 s");
                }
            }
        });
    match spawned {
        Ok(handle) => {
            *WD_THREAD.lock().unwrap_or_else(|e| e.into_inner()) = Some(handle);
            match rx.recv_timeout(Duration::from_secs(3)) {
                Ok(msg) => info("watchdog", msg),
                Err(_) => warn("watchdog", "the watchdog thread didn't start within 3 s"),
            }
        }
        Err(e) => warn(
            "watchdog",
            format!("couldn't start the watchdog thread: {e}"),
        ),
    }
}

/// Stops and joins the watchdog. main must do this before returning: the loader unmaps the NRO
/// once main returns, and a thread that wakes up after that runs unmapped code. (Bevy's pool
/// threads are parked with no timeout, so they never wake.)
pub fn stop_watchdog() {
    WD_STOP.store(true, Ordering::SeqCst);
    let handle = WD_THREAD.lock().unwrap_or_else(|e| e.into_inner()).take();
    if let Some(handle) = handle {
        let _ = handle.join();
    }
}

/// Chains a panic hook in front of bevy_horizon's (which writes panic.txt), so call it after
/// `init_switch_logging`. Panics abort, and the exit path after an abort can outlast the
/// running test's time limit: the watchdog firing then would use files and locks that are being
/// torn down. So the hook stops the watchdog first, then adds a PANIC line to the results. The
/// crash marker stays, so the next run records the test as CRASH and skips it.
pub fn install_panic_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        WD_STOP.store(true, Ordering::SeqCst);
        disarm();
        stop_frame_watch();
        let mut id = running();
        if id.is_empty() {
            id = "(no test running)".into();
        }
        let message = info
            .payload()
            .downcast_ref::<&str>()
            .map(|s| s.to_string())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "(no message)".into())
            .replace('\n', " ");
        let at = info
            .location()
            .map(|l| format!(" at {}:{}", l.file(), l.line()))
            .unwrap_or_default();
        let line = format!(
            "{:7.1}s PANIC {id:<28} {message}{at}; the app aborted (details in panic.txt). The next run skips this test\n",
            elapsed()
        );
        if let Ok(mut f) = OpenOptions::new()
            .append(true)
            .open(path("captest_results.txt"))
        {
            let _ = f.write_all(line.as_bytes());
            let _ = f.sync_all();
        }
        previous(info);
    }));
}

/// The test that was running (the marker file), if any.
pub fn running() -> String {
    fs::read_to_string(path("captest_running.txt"))
        .unwrap_or_default()
        .trim()
        .to_string()
}

/// Ends the whole process, not just main: `exit` would return to hbloader with the stuck thread
/// still running in memory hbloader then reuses. The user lands on the HOME menu.
///
/// Does nothing once the watchdog is stopped: main is exiting or a panic is aborting the app,
/// and the exit path owns the files and the process from then on.
fn fire(what: &str) {
    if WD_STOP.load(Ordering::SeqCst) {
        return;
    }
    let id = WD_ID
        .try_lock()
        .map(|s| s.clone())
        .unwrap_or_else(|_| "unknown".into());
    let line = format!(
        "{:7.1}s HANG  {id:<28} {what}; the watchdog closed the app. The next run skips this test\n",
        elapsed()
    );
    if let Ok(mut f) = OpenOptions::new()
        .append(true)
        .open(path("captest_results.txt"))
    {
        let _ = f.write_all(line.as_bytes());
        let _ = f.sync_all();
    }
    unsafe {
        nx::write(1, line.as_ptr().cast(), line.len());
        nx::svcExitProcess()
    }
}
