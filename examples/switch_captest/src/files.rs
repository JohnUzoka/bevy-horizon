//! The SD card through std::fs (libnx's fsdev over FAT32/exFAT), the NRO's romfs, and memmap2.

use std::fs::{self, File, OpenOptions};
use std::io::{ErrorKind, Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::time::{Instant, SystemTime};

use crate::nx;
use crate::report::{self, check, check_warn, fail, info, warn};

fn block(i: usize) -> Vec<u8> {
    (0..1usize << 20)
        .map(|j| (j.wrapping_mul(7) + i * 13 + 3) as u8)
        .collect()
}

fn mbps(bytes: usize, secs: f64) -> f64 {
    bytes as f64 / (1 << 20) as f64 / secs.max(1e-6)
}

/// Writes then reads back `mib` MiB, returning (write MB/s, read MB/s) or what failed.
fn throughput(path: &str, mib: usize) -> Result<(f64, f64), String> {
    let blocks: Vec<Vec<u8>> = (0..4).map(block).collect();
    let t = Instant::now();
    let mut f = File::create(path).map_err(|e| format!("create: {e}"))?;
    for i in 0..mib {
        f.write_all(&blocks[i % 4])
            .map_err(|e| format!("write block {i}: {e}"))?;
    }
    f.sync_all().map_err(|e| format!("sync_all: {e}"))?;
    drop(f);
    let write = mbps(mib << 20, t.elapsed().as_secs_f64());
    let t = Instant::now();
    let read = read_verify(path, mib)?;
    Ok((write, mbps(read, t.elapsed().as_secs_f64())))
}

fn read_verify(path: &str, mib: usize) -> Result<usize, String> {
    let blocks: Vec<Vec<u8>> = (0..4).map(block).collect();
    let mut f = File::open(path).map_err(|e| format!("open: {e}"))?;
    let mut buf = vec![0u8; 1 << 20];
    for i in 0..mib {
        f.read_exact(&mut buf)
            .map_err(|e| format!("read block {i}: {e}"))?;
        if buf != blocks[i % 4] {
            return Err(format!("block {i} read back different"));
        }
    }
    Ok(mib << 20)
}

fn kind_check(id: &str, what: &str, result: std::io::Result<()>, want: ErrorKind) {
    match result {
        Ok(()) => warn(id, format!("{what}: succeeded, expected {want:?}")),
        Err(e) => {
            check_warn(
                id,
                e.kind() == want,
                format!("{what}: {:?} ({e}), desktop gives {want:?}", e.kind()),
            );
        }
    }
}

pub fn sd_card() {
    report::heading("SD card");
    let dir = report::path("captest_tmp");
    if let Err(e) = fs::remove_dir_all(&dir)
        && e.kind() != ErrorKind::NotFound
    {
        info(
            "fs-stale-dir",
            format!(
                "an earlier run's {dir} won't go ({e}); it holds {:?}",
                listing(&dir)
            ),
        );
    }
    if let Err(e) = fs::create_dir_all(&dir) {
        fail("fs-tmp-dir", format!("can't create {dir}: {e}"));
        return;
    }
    report::section("fs-throughput", 120, || {
        for mib in [1usize, 16, 64] {
            match throughput(&format!("{dir}/big_{mib}.bin"), mib) {
                Ok((w, r)) => info(
                    &format!("fs-speed-{mib}mib"),
                    format!("{mib} MiB: write {w:.1} MB/s (with sync), read {r:.1} MB/s, verified"),
                ),
                Err(e) => fail(&format!("fs-speed-{mib}mib"), e),
            }
        }
        let t = Instant::now();
        let small = (|| -> std::io::Result<()> {
            let mut f = File::create(format!("{dir}/small.bin"))?;
            for i in 0..256u32 {
                f.write_all(&[i as u8; 4096])?;
            }
            f.sync_all()
        })();
        match small {
            Ok(()) => info(
                "fs-small-writes",
                format!(
                    "256 x 4 KiB writes: {:.1} ms",
                    t.elapsed().as_secs_f64() * 1e3
                ),
            ),
            Err(e) => fail("fs-small-writes", e.to_string()),
        }
    });
    report::section("fs-concurrent-read", 60, || {
        let path = format!("{dir}/big_16.bin");
        let t = Instant::now();
        let readers: Vec<_> = (0..3)
            .map(|_| {
                let path = path.clone();
                std::thread::spawn(move || read_verify(&path, 16))
            })
            .collect();
        let results: Vec<_> = readers
            .into_iter()
            .map(|h| h.join().unwrap_or(Err("panicked".into())))
            .collect();
        let ok = results.iter().all(|r| r.is_ok());
        check(
            "fs-concurrent-read",
            ok,
            format!(
                "3 threads reading 16 MiB each: {:.1} MB/s total{}",
                mbps(48 << 20, t.elapsed().as_secs_f64()),
                results
                    .iter()
                    .find_map(|r| r.as_ref().err())
                    .map(|e| format!(", error: {e}"))
                    .unwrap_or_default()
            ),
        );
    });
    report::section("fs-semantics", 60, || semantics(&dir));
    report::section("fs-names", 30, || names(&dir));
    report::section("fs-errors", 30, || {
        kind_check(
            "fs-err-missing",
            "open a missing file",
            File::open(format!("{dir}/nope.txt")).map(drop),
            ErrorKind::NotFound,
        );
        kind_check(
            "fs-err-exists",
            "create_dir on an existing dir",
            fs::create_dir(&dir),
            ErrorKind::AlreadyExists,
        );
        kind_check(
            "fs-err-missing-parent",
            "create in a missing dir",
            File::create(format!("{dir}/no/such.txt")).map(drop),
            ErrorKind::NotFound,
        );
        kind_check(
            "fs-err-dir-not-empty",
            "remove_dir on a non-empty dir",
            fs::remove_dir(&dir),
            ErrorKind::DirectoryNotEmpty,
        );
        // Horizon's FS opens only files, and says PathNotFound for a folder, which libnx's
        // fsdev turns into ENOENT. Turning that into EISDIR would take another FS call on every
        // failed open, and Bevy fails one for each asset that has no .meta file, so it's left.
        match fs::read(&dir) {
            Err(e) if e.kind() == ErrorKind::NotFound => info(
                "fs-err-read-dir-as-file",
                format!(
                    "fs::read on a directory: NotFound ({e}), where desktops give IsADirectory; Horizon behaviour, left as it is"
                ),
            ),
            result => kind_check(
                "fs-err-read-dir-as-file",
                "fs::read on a directory",
                result.map(drop),
                ErrorKind::IsADirectory,
            ),
        }
        let _ = fs::write(format!("{dir}/bad_utf8.bin"), [0xFFu8, 0xFE, 0x80]);
        kind_check(
            "fs-err-invalid-utf8",
            "read_to_string on invalid UTF-8",
            fs::read_to_string(format!("{dir}/bad_utf8.bin")).map(drop),
            ErrorKind::InvalidData,
        );
    });
    report::section("fs-environment", 30, || environment(&dir));
    match fs::remove_dir_all(&dir) {
        Ok(()) => {}
        Err(e) => warn(
            "fs-cleanup",
            format!(
                "remove_dir_all({dir}) failed: {e}; still there: {:?}",
                listing(&dir)
            ),
        ),
    }
}

/// A file name with anything but printable ASCII as `\xNN`, to show the bytes the SD card gave.
fn escaped(name: &std::ffi::OsStr) -> String {
    use std::os::unix::ffi::OsStrExt;
    name.as_bytes().escape_ascii().to_string()
}

/// `name` escaped, and cut short past 40 characters: one test's file name is 204 long.
fn short(name: &[u8]) -> String {
    let s = name.escape_ascii().to_string();
    if s.len() > 48 {
        format!("{}... ({} bytes)", &s[..40], name.len())
    } else {
        s
    }
}

/// The names in `dir`, escaped.
fn listing(dir: &str) -> Vec<String> {
    fs::read_dir(dir)
        .map(|rd| rd.filter_map(|e| e.ok()).map(|e| escaped(&e.file_name())).collect())
        .unwrap_or_default()
}

fn semantics(dir: &str) {
    let (a, b) = (format!("{dir}/a.txt"), format!("{dir}/b.txt"));
    let _ = fs::write(&a, "A");
    let _ = fs::write(&b, "B");
    // Horizon refuses to rename onto an existing file; bevy_horizon's `rename` replaces it.
    match fs::rename(&a, &b) {
        Ok(()) => {
            let aside = Path::new(&format!("{b}.bh-replaced")).exists();
            check(
                "fs-rename-over",
                fs::read_to_string(&b).ok().as_deref() == Some("A")
                    && !Path::new(&a).exists()
                    && !aside,
                format!(
                    "rename onto an existing file replaces it (source gone: {}, nothing left aside: {})",
                    !Path::new(&a).exists(),
                    !aside
                ),
            );
        }
        Err(e) => fail(
            "fs-rename-over",
            format!("rename onto an existing file fails ({e}): bevy_horizon's __wrap_rename isn't linked (the target spec needs --wrap=rename)"),
        ),
    };
    // POSIX makes this a no-op. The replacing `rename` must not lose the file either way.
    let same = fs::rename(&b, &b);
    check(
        "fs-rename-same",
        fs::read_to_string(&b).ok().as_deref() == Some("A"),
        format!("rename of a file onto itself: {same:?}, contents kept"),
    );
    let (d1, d2) = (format!("{dir}/dir1"), format!("{dir}/dir2"));
    let renamed = fs::create_dir_all(format!("{d1}/x/y")).and_then(|()| fs::rename(&d1, &d2));
    check(
        "fs-rename-dir",
        renamed.is_ok() && Path::new(&format!("{d2}/x/y")).is_dir(),
        format!("rename a directory tree: {renamed:?}"),
    );
    let p = format!("{dir}/seek.bin");
    let r = (|| -> std::io::Result<String> {
        fs::write(&p, (0..100u8).collect::<Vec<_>>())?;
        let mut f = OpenOptions::new().read(true).write(true).open(&p)?;
        f.seek(SeekFrom::End(-10))?;
        let mut tail = [0u8; 10];
        f.read_exact(&mut tail)?;
        f.seek(SeekFrom::Start(50))?;
        f.write_all(b"XY")?;
        f.set_len(200)?;
        let grown = f.metadata()?.len();
        f.set_len(20)?;
        let shrunk = f.metadata()?.len();
        drop(f);
        OpenOptions::new()
            .append(true)
            .open(&p)?
            .write_all(b"END")?;
        let data = fs::read(&p)?;
        let ok = tail[0] == 90
            && grown == 200
            && shrunk == 20
            && data.len() == 23
            && data.ends_with(b"END");
        Ok(format!(
            "{} seek/tail {:?}, set_len 200 -> {grown}, 20 -> {shrunk}, append -> {} bytes",
            if ok { "ok:" } else { "MISMATCH:" },
            &tail[..2],
            data.len()
        ))
    })();
    match r {
        Ok(msg) => {
            check("fs-seek-truncate-append", msg.starts_with("ok"), msg);
        }
        Err(e) => fail("fs-seek-truncate-append", e.to_string()),
    };
    // std's copy creates and truncates the destination, then sets its permissions with fchmod,
    // which libnx doesn't have: without bevy_horizon's --wrap=fchmod it leaves an empty file.
    let copied = fs::copy(format!("{dir}/big_1.bin"), format!("{dir}/copy.bin"));
    let same = matches!(
        (fs::read(format!("{dir}/big_1.bin")), fs::read(format!("{dir}/copy.bin"))),
        (Ok(a), Ok(b)) if a == b
    );
    check(
        "fs-copy",
        matches!(copied, Ok(n) if n == 1 << 20) && same,
        format!(
            "fs::copy of 1 MiB: {copied:?}; {}",
            if same { "the copy matches" } else { "the copy differs from the source" }
        ),
    );

    let many = format!("{dir}/many");
    let t = Instant::now();
    let made = (|| -> std::io::Result<usize> {
        fs::create_dir(&many)?;
        for i in 0..200 {
            fs::write(format!("{many}/f{i:03}.txt"), i.to_string())?;
        }
        Ok(fs::read_dir(&many)?.count())
    })();
    let create_ms = t.elapsed().as_secs_f64() * 1e3;
    let t = Instant::now();
    let removed = fs::remove_dir_all(&many);
    check(
        "fs-200-files",
        matches!(made, Ok(200)) && removed.is_ok(),
        format!(
            "200 files: created+listed {made:?} in {create_ms:.0} ms, removed in {:.0} ms",
            t.elapsed().as_secs_f64() * 1e3
        ),
    );

    match fs::metadata(format!("{dir}/big_16.bin")) {
        Ok(m) => {
            check(
                "fs-metadata",
                m.is_file() && m.len() == 16 << 20,
                format!("len {} is_file {}", m.len(), m.is_file()),
            );
            match m.modified() {
                Ok(t) => {
                    let now = SystemTime::now();
                    let diff = match now.duration_since(t) {
                        Ok(d) => d.as_secs() as i64,
                        Err(e) => -(e.duration().as_secs() as i64),
                    };
                    check_warn(
                        "fs-mtime",
                        diff.abs() < 300,
                        format!(
                            "a file written seconds ago has mtime {diff} s before now (an hours-sized offset is the timezone: FAT stores local time)"
                        ),
                    );
                }
                Err(e) => warn("fs-mtime", format!("modified() fails: {e}")),
            }
        }
        Err(e) => fail("fs-metadata", e.to_string()),
    }
    let ro = format!("{dir}/readonly.txt");
    let _ = fs::write(&ro, "x");
    if let Ok(m) = fs::metadata(&ro) {
        let mut perms = m.permissions();
        perms.set_readonly(true);
        let set = fs::set_permissions(&ro, perms);
        let now_ro = fs::metadata(&ro)
            .map(|m| m.permissions().readonly())
            .unwrap_or(false);
        let write = OpenOptions::new().write(true).open(&ro).map(drop);
        info(
            "fs-readonly",
            format!(
                "set_readonly: {set:?}, reads back readonly={now_ro}, opening for write: {write:?}"
            ),
        );
        if let Ok(m) = fs::metadata(&ro) {
            let mut perms = m.permissions();
            #[allow(clippy::permissions_set_readonly_false)]
            perms.set_readonly(false);
            let _ = fs::set_permissions(&ro, perms);
        }
    }
}

fn names(dir: &str) {
    let long = format!("{dir}/{}.txt", "n".repeat(200));
    check_warn(
        "fs-long-name",
        fs::write(&long, "x").is_ok() && fs::read(&long).is_ok(),
        "a 204-character file name",
    );
    let unicode_name = "caf\u{e9}_\u{65e5}\u{672c}_\u{f1}.txt";
    let unicode_path = format!("{dir}/{unicode_name}");
    let created = fs::write(&unicode_path, "x");
    let mut names = Vec::new();
    let mut list_error = String::new();
    match fs::read_dir(dir) {
        Ok(entries) => {
            for entry in entries {
                match entry {
                    Ok(e) => names.push(e.file_name()),
                    Err(e) => list_error = format!(", then failed: {e}"),
                }
            }
        }
        Err(e) => list_error = format!(", then failed: {e}"),
    }
    let listed = names.iter().any(|n| n.to_string_lossy() == unicode_name);
    let read = fs::read(&unicode_path).map(|data| data == b"x");
    let mut detail = format!(
        "non-ASCII file name: create {created:?}, listed back {listed}, read by name {read:?}"
    );
    // The same folder from the FS service itself, beneath libnx's fsdev and newlib's readdir.
    // Run 5 on hardware didn't list the name at all; Ryujinx always does.
    let raw = nx::fs_list_raw(dir.strip_prefix("sdmc:").unwrap_or(dir));
    let raw_count = match raw.count {
        Ok(n) => n.to_string(),
        Err(rc) => format!("unknown (0x{rc:x})"),
    };
    let raw_error = raw
        .error
        .as_ref()
        .map(|e| format!(", then failed: {e}"))
        .unwrap_or_default();
    let raw_has = raw.entries.iter().any(|(n, _)| n == unicode_name.as_bytes());
    info(
        "fs-list-raw",
        format!(
            "the FS service counts {raw_count} entries in the folder and lists {}{raw_error}{}; read_dir lists {}{list_error}",
            raw.entries.len(),
            if raw_has { ", the non-ASCII name among them" } else { "" },
            names.len(),
        ),
    );
    if created.is_ok() && !listed {
        use std::os::unix::ffi::OsStrExt;
        let std_names: Vec<_> = names.iter().map(|n| short(n.as_bytes())).collect();
        let raw_names: Vec<_> = raw
            .entries
            .iter()
            .map(|(n, is_dir)| format!("{}{}", short(n), if *is_dir { "/" } else { "" }))
            .collect();
        detail += &format!(
            "; looking for {}, read_dir lists {std_names:?}{list_error}; the FS service lists {raw_names:?}{raw_error}",
            short(unicode_name.as_bytes()),
        );
    }
    check_warn("fs-unicode-name", created.is_ok() && listed, detail);
    if created.is_ok() {
        info(
            "fs-unicode-remove",
            format!("remove_file by its name: {:?}", fs::remove_file(&unicode_path)),
        );
    }
    for bad in [
        "bad:name.txt",
        "bad*name.txt",
        "bad?name.txt",
        "bad\"name.txt",
    ] {
        let r = fs::write(format!("{dir}/{bad}"), "x");
        info(
            "fs-invalid-name",
            format!(
                "{bad}: {}",
                match &r {
                    Ok(()) => "accepted".into(),
                    Err(e) => format!("{:?} ({e})", e.kind()),
                }
            ),
        );
    }
    let _ = fs::write(format!("{dir}/CaseTest.txt"), "x");
    let insensitive = Path::new(&format!("{dir}/casetest.txt")).exists();
    info(
        "fs-case",
        if insensitive {
            "file names are case-insensitive (FAT): Foo.png and foo.png are the same file"
        } else {
            "file names are case-sensitive"
        },
    );
}

fn environment(dir: &str) {
    info("env-current-dir", format!("{:?}", std::env::current_dir()));
    let rel = fs::write("captest_relative.txt", "x");
    let landed = Path::new(&report::path("captest_relative.txt")).exists();
    info(
        "env-relative-path",
        format!("writing \"captest_relative.txt\": {rel:?}; lands in the NRO folder: {landed}"),
    );
    let _ = fs::remove_file("captest_relative.txt");
    let name = dir.rsplit('/').next().unwrap_or("captest_tmp");
    info(
        "env-canonicalize",
        format!("{:?}", fs::canonicalize(format!("{dir}/../{name}"))),
    );
    match std::env::current_exe() {
        Ok(p) => info("env-current-exe", format!("{p:?}")),
        Err(e) => info(
            "env-current-exe",
            format!("fails ({e}); bevy_horizon sets BEVY_ASSET_ROOT so Bevy never needs it"),
        ),
    }
    let tmp = std::env::temp_dir().join("captest_tmpfile.txt");
    let wrote = fs::write(&tmp, "x");
    info(
        "env-temp-dir",
        format!(
            "temp_dir() = {:?}; writing there: {wrote:?}",
            std::env::temp_dir()
        ),
    );
    let _ = fs::remove_file(&tmp);
    info(
        "env-args",
        format!("{:?}", std::env::args().collect::<Vec<_>>()),
    );
    unsafe { std::env::set_var("CAPTEST_VAR", "42") };
    check(
        "env-vars",
        std::env::var("CAPTEST_VAR").ok().as_deref() == Some("42"),
        "set_var/var round trip",
    );
    let mut buf = [0u64; 16];
    let rc = unsafe { nx::statvfs(c"sdmc:/".as_ptr(), buf.as_mut_ptr()) };
    if rc == 0 {
        let gib = |blocks: u64| (blocks * buf[1]) as f64 / (1u64 << 30) as f64;
        info(
            "fs-statvfs",
            format!(
                "SD card {:.1} GiB, {:.1} GiB free",
                gib(buf[2]),
                gib(buf[4])
            ),
        );
    } else {
        warn("fs-statvfs", format!("statvfs(sdmc:/) returned {rc}"));
    }
}

pub fn romfs() {
    report::heading("romfs");
    report::section("romfs", 30, || {
        match fs::read_to_string("romfs:/captest.txt") {
            Ok(s) => {
                check(
                    "romfs-read",
                    s == "switch_captest romfs OK\n",
                    format!("romfs:/captest.txt = {s:?}"),
                );
            }
            Err(e) => fail(
                "romfs-read",
                format!("romfs:/captest.txt: {e} (was the NRO built with --romfsdir?)"),
            ),
        };
        match fs::read("romfs:/data/blob.bin") {
            Ok(b) => {
                let ok = b.len() == 65536
                    && b.iter()
                        .enumerate()
                        .all(|(i, &x)| x == ((i * 7 + 3) % 256) as u8);
                check(
                    "romfs-blob",
                    ok,
                    format!(
                        "64 KiB binary file, {} bytes, pattern {}",
                        b.len(),
                        if ok { "ok" } else { "WRONG" }
                    ),
                );
            }
            Err(e) => fail("romfs-blob", e.to_string()),
        };
        let names: Vec<String> = fs::read_dir("romfs:/")
            .map(|rd| {
                rd.filter_map(|e| e.ok())
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default();
        check(
            "romfs-read-dir",
            names.iter().any(|n| n == "data"),
            format!("romfs:/ lists {names:?}"),
        );
        let deep = fs::read_to_string("romfs:/data/nested/deep.txt");
        check(
            "romfs-nested",
            deep.as_deref().ok() == Some("nested romfs OK\n"),
            format!("nested file: {deep:?}"),
        );
        check(
            "romfs-metadata",
            fs::metadata("romfs:/data/blob.bin").map(|m| m.len()).ok() == Some(65536),
            "metadata().len()",
        );
        let write = fs::write("romfs:/new.txt", "x");
        check(
            "romfs-read-only",
            write.is_err(),
            format!("writing to romfs fails as it should: {write:?}"),
        );
        kind_check(
            "romfs-not-found",
            "open a missing romfs file",
            File::open("romfs:/missing.bin").map(drop),
            ErrorKind::NotFound,
        );
    });
}

/// Copies romfs:/assets/* to the NRO's assets folder if it isn't there, so the SD asset tests
/// work even if only the .nro was copied.
pub fn install_assets() {
    let target = report::path("assets");
    let Ok(entries) = fs::read_dir("romfs:/assets") else {
        warn("assets-install", "romfs:/assets is missing from this build");
        return;
    };
    let _ = fs::create_dir_all(&target);
    let mut copied = Vec::new();
    for entry in entries.filter_map(|e| e.ok()) {
        let name = entry.file_name().to_string_lossy().into_owned();
        let dest = format!("{target}/{name}");
        // Compare sizes, not just existence: a failed copy leaves an empty file behind.
        let want = entry.metadata().map(|m| m.len()).ok();
        let have = fs::metadata(&dest).map(|m| m.len()).ok();
        if have.is_none() || have != want {
            match fs::copy(entry.path(), &dest) {
                Ok(_) => copied.push(name),
                Err(e) => warn("assets-install", format!("copying {name} to {dest}: {e}")),
            }
        }
    }
    if copied.is_empty() {
        info(
            "assets-install",
            format!("{target} already has the test assets"),
        );
    } else {
        info(
            "assets-install",
            format!("copied {copied:?} from romfs to {target}"),
        );
    }
}

pub fn memmap() {
    report::section("memmap2", 10, || {
        let path = report::path("assets/captest.wav");
        let (f, whole) = match File::open(&path).and_then(|f| Ok((f, fs::read(&path)?))) {
            Ok(opened) => opened,
            Err(e) => return warn("memmap2", format!("can't read {path}: {e}")),
        };
        let mut f = f;
        match unsafe { memmap2::Mmap::map(&f) } {
            Ok(m) => {
                let position = f.stream_position();
                check(
                    "memmap2",
                    m[..] == whole[..] && whole.starts_with(b"RIFF") && matches!(position, Ok(0)),
                    format!(
                        "Mmap::map: {} bytes, {} what fs::read gives; the file position after is {position:?}",
                        m.len(),
                        if m[..] == whole[..] { "the same as" } else { "different from" },
                    ),
                );
            }
            Err(e) => warn(
                "memmap2",
                format!(
                    "Mmap::map fails ({e}): crates that mmap files need a read-into-memory fallback"
                ),
            ),
        }
        // Part of a file, as crates that map one entry of an archive do. Bytes 8..12 of a WAV
        // file are "WAVE".
        let part = unsafe { memmap2::MmapOptions::new().offset(8).len(4).map(&f) };
        check(
            "memmap2-offset",
            part.as_ref().is_ok_and(|m| m[..] == *b"WAVE"),
            format!(
                "offset 8, length 4: {:?}",
                part.as_ref()
                    .map(|m| String::from_utf8_lossy(&m[..]).into_owned())
            ),
        );
        // A private (copy-on-write) map takes writes without changing the file.
        let copy = unsafe { memmap2::MmapOptions::new().map_copy(&f) }.map(|mut m| {
            m[..4].copy_from_slice(b"XXXX");
            m[..4] == *b"XXXX"
        });
        let file_after = fs::read(&path).map(|d| d.starts_with(b"RIFF"));
        check(
            "memmap2-copy",
            matches!(copy, Ok(true)) && matches!(file_after, Ok(true)),
            format!("map_copy then a write: {copy:?}; the file still starts with RIFF: {file_after:?}"),
        );
        let anon = memmap2::MmapOptions::new()
            .len(4 << 20)
            .map_anon()
            .map(|mut m| {
                let zeroed = m.iter().all(|&b| b == 0);
                m[(4 << 20) - 1] = 7;
                zeroed && m[(4 << 20) - 1] == 7
            });
        check(
            "memmap2-anon",
            matches!(anon, Ok(true)),
            format!("a 4 MiB anonymous map is zeroed and writable: {anon:?}"),
        );
        let shared = unsafe { memmap2::MmapMut::map_mut(&f) }.map(drop);
        info(
            "memmap2-shared-write",
            format!("map_mut, a writable map shared with its file: {shared:?}"),
        );
    });
}
