//! Crates IW4L and Skate use that build C or assembly (zstd, Lua, ring) or lean on the platform
//! (tokio's threads and timers), plus Bevy's own task pools. Each runs with the app already built,
//! so romfs is mounted and sockets are up.

#![allow(unused_imports)]

use std::io::{Cursor, Read, Write};
use std::time::{Duration, Instant};

use crate::nx;
use crate::platform::{calibrate, spin};
use crate::report::{self, check, check_warn, fail, info, pass, warn};

/// 4 MiB that compresses about as well as game data does.
fn sample(len: usize) -> Vec<u8> {
    let mut x = 0x2545_F491_4F6C_DD1Du64;
    (0..len)
        .map(|i| {
            if i % 512 < 384 {
                ((i / 64) % 251) as u8
            } else {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                x as u8
            }
        })
        .collect()
}

fn mbs(bytes: usize, t: Instant) -> f64 {
    bytes as f64 / t.elapsed().as_secs_f64().max(1e-6) / 1e6
}

pub fn native() {
    report::heading("Native crates");
    #[cfg(feature = "flate2")]
    report::section("crate-flate2", 30, flate2_test);
    #[cfg(feature = "zip")]
    report::section("crate-zip", 30, zip_test);
    #[cfg(feature = "zstd")]
    report::section("crate-zstd", 30, zstd_test);
    #[cfg(feature = "ring")]
    report::section("crate-ring", 30, ring_test);
    #[cfg(feature = "lua")]
    report::section("crate-mlua", 30, lua_test);
    #[cfg(feature = "tokio")]
    report::section("crate-tokio", 30, tokio_test);
    #[cfg(feature = "symphonia")]
    report::section("crate-symphonia", 30, symphonia_test);
    #[cfg(feature = "bcdec")]
    report::section("crate-bcdec", 10, bcdec_test);
    #[cfg(feature = "image")]
    report::section("crate-image", 30, image_test);
    #[cfg(not(feature = "native"))]
    info(
        "native",
        "built without the `native` feature; only the crates enabled one by one ran",
    );
}

#[cfg(feature = "flate2")]
fn flate2_test() {
    use flate2::{Compression, read::ZlibDecoder, write::ZlibEncoder};
    let data = sample(4 << 20);
    let t = Instant::now();
    let enc = (|| {
        let mut e = ZlibEncoder::new(Vec::new(), Compression::default());
        e.write_all(&data)?;
        e.finish()
    })();
    let enc = match enc {
        Ok(v) => v,
        Err(e) => return fail("crate-flate2", format!("compress: {e}")),
    };
    let ct = mbs(data.len(), t);
    let t = Instant::now();
    let mut out = Vec::with_capacity(data.len());
    let r = ZlibDecoder::new(&enc[..]).read_to_end(&mut out);
    check(
        "crate-flate2",
        r.is_ok() && out == data,
        format!(
            "zlib-rs: 4 MiB -> {} KiB, compress {ct:.0} MB/s, decompress {:.0} MB/s",
            enc.len() >> 10,
            mbs(data.len(), t)
        ),
    );
}

#[cfg(feature = "zip")]
fn zip_test() {
    use zip::{CompressionMethod, ZipArchive, ZipWriter, write::SimpleFileOptions};
    let data = sample(256 * 1024);
    let built = (|| -> Result<Vec<u8>, Box<dyn std::error::Error>> {
        let mut w = ZipWriter::new(Cursor::new(Vec::new()));
        let opts = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
        w.start_file("hello.txt", opts)?;
        w.write_all(b"hello from the switch")?;
        w.add_directory("maps/", SimpleFileOptions::default())?;
        w.start_file("maps/data.bin", opts)?;
        w.write_all(&data)?;
        Ok(w.finish()?.into_inner())
    })();
    let bytes = match built {
        Ok(b) => b,
        Err(e) => return fail("crate-zip", format!("writing an archive: {e}")),
    };
    fn read<R: Read + std::io::Seek>(
        archive: &mut ZipArchive<R>,
    ) -> Result<(String, Vec<u8>), Box<dyn std::error::Error>> {
        let mut s = String::new();
        archive.by_name("hello.txt")?.read_to_string(&mut s)?;
        let mut d = Vec::new();
        archive.by_name("maps/data.bin")?.read_to_end(&mut d)?;
        Ok((s, d))
    }
    let mem = ZipArchive::new(Cursor::new(bytes.clone()))
        .map_err(Into::into)
        .and_then(|mut a| read(&mut a));
    check(
        "crate-zip-memory",
        matches!(&mem, Ok((s, d)) if s == "hello from the switch" && *d == data),
        format!(
            "deflate archive in memory, {} KiB: {}",
            bytes.len() >> 10,
            mem.as_ref()
                .map(|_| "read back".to_string())
                .unwrap_or_else(|e| e.to_string())
        ),
    );
    // Archives on the SD card are read through File's Seek, the way games read .pak/.zip files.
    let path = report::path("captest_tmp.zip");
    let sd = std::fs::write(&path, &bytes)
        .map_err(Into::into)
        .and_then(|()| std::fs::File::open(&path).map_err(Into::into))
        .and_then(|f| ZipArchive::new(std::io::BufReader::new(f)).map_err(Into::into))
        .and_then(|mut a| read(&mut a));
    let _ = std::fs::remove_file(&path);
    check(
        "crate-zip-sd",
        matches!(&sd, Ok((s, d)) if s == "hello from the switch" && *d == data),
        format!(
            "the same archive read from the SD card: {}",
            sd.as_ref()
                .map(|_| "ok".to_string())
                .unwrap_or_else(|e| e.to_string())
        ),
    );
}

#[cfg(feature = "zstd")]
fn zstd_test() {
    let data = sample(4 << 20);
    let t = Instant::now();
    let enc = match zstd::encode_all(&data[..], 3) {
        Ok(v) => v,
        Err(e) => return fail("crate-zstd", format!("compress: {e}")),
    };
    let ct = mbs(data.len(), t);
    let t = Instant::now();
    let dec = zstd::decode_all(&enc[..]);
    check(
        "crate-zstd",
        dec.as_ref().is_ok_and(|d| *d == data),
        format!(
            "C zstd level 3: 4 MiB -> {} KiB, compress {ct:.0} MB/s, decompress {:.0} MB/s",
            enc.len() >> 10,
            mbs(data.len(), t)
        ),
    );
    let bad = zstd::decode_all(&b"definitely not zstd"[..]);
    check(
        "crate-zstd-corrupt",
        bad.is_err(),
        "corrupt input returns an error instead of crashing",
    );
}

#[cfg(feature = "ring")]
fn ring_test() {
    use ring::aead::{AES_256_GCM, Aad, CHACHA20_POLY1305, LessSafeKey, Nonce, UnboundKey};
    use ring::digest::{SHA256, digest};
    use ring::rand::{SecureRandom, SystemRandom};
    let hex: String = digest(&SHA256, b"abc")
        .as_ref()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    check(
        "crate-ring-sha256",
        hex == "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
        format!("SHA-256(\"abc\") = {}...", &hex[..16]),
    );
    let rng = SystemRandom::new();
    let (mut key, mut nonce) = ([0u8; 32], [0u8; 12]);
    let filled = rng.fill(&mut key).and_then(|()| rng.fill(&mut nonce));
    check(
        "crate-ring-random",
        filled.is_ok() && key != [0; 32],
        "SystemRandom (getrandom 0.2 -> libnx randomGet) filled a key",
    );
    for (name, alg) in [
        ("AES-256-GCM", &AES_256_GCM),
        ("ChaCha20-Poly1305", &CHACHA20_POLY1305),
    ] {
        let id = if name.starts_with("AES") {
            "crate-ring-aes-gcm"
        } else {
            "crate-ring-chacha"
        };
        let Ok(unbound) = UnboundKey::new(alg, &key) else {
            return fail(id, "UnboundKey::new failed");
        };
        let k = LessSafeKey::new(unbound);
        let mut buf = sample(1 << 20);
        let plain = buf.clone();
        let t = Instant::now();
        let sealed =
            k.seal_in_place_append_tag(Nonce::assume_unique_for_key(nonce), Aad::empty(), &mut buf);
        let speed = mbs(plain.len(), t);
        let opened = sealed.ok().and_then(|()| {
            k.open_in_place(Nonce::assume_unique_for_key(nonce), Aad::empty(), &mut buf)
                .ok()
                .map(|p| p.to_vec())
        });
        let mut tampered = plain.clone();
        let _ = k.seal_in_place_append_tag(
            Nonce::assume_unique_for_key(nonce),
            Aad::empty(),
            &mut tampered,
        );
        tampered[100] ^= 1;
        let rejected = k
            .open_in_place(
                Nonce::assume_unique_for_key(nonce),
                Aad::empty(),
                &mut tampered,
            )
            .is_err();
        check(
            id,
            opened.as_deref() == Some(&plain[..]) && rejected,
            format!("{name}: 1 MiB round trip, tampering rejected, seal {speed:.0} MB/s"),
        );
    }
    let big = sample(16 << 20);
    let t = Instant::now();
    let _ = digest(&SHA256, &big);
    info(
        "crate-ring-speed",
        format!(
            "SHA-256 {:.0} MB/s (fast means ring uses the ARMv8 crypto instructions)",
            mbs(big.len(), t)
        ),
    );
}

#[cfg(feature = "lua")]
fn lua_test() {
    let lua = mlua::Lua::new();
    let r = lua.load("return 6 * 7").eval::<i64>();
    check(
        "crate-mlua-eval",
        matches!(r, Ok(42)),
        format!("vendored Lua 5.4 `return 6 * 7` = {r:?}"),
    );
    let r = lua
        .create_function(|_, (a, b): (i64, i64)| Ok(a + b))
        .and_then(|f| lua.globals().set("add", f))
        .and_then(|()| lua.load("return add(40, 2)").eval::<i64>());
    check(
        "crate-mlua-rust-fn",
        matches!(r, Ok(42)),
        format!("Lua calling a Rust function: {r:?}"),
    );
    // error() longjmps out of the interpreter; mlua turns it into an Err at the pcall boundary.
    let e = lua.load("error('boom')").exec();
    let first = e
        .as_ref()
        .err()
        .map(|e| e.to_string().lines().next().unwrap_or_default().to_string())
        .unwrap_or_default();
    check(
        "crate-mlua-error",
        first.contains("boom"),
        format!("error('boom') became a Rust Err: {first:?}"),
    );
    let r = lua.load("local ok, msg = pcall(function() error('inner') end) return ok == false and msg:find('inner') ~= nil").eval::<bool>();
    check(
        "crate-mlua-pcall",
        matches!(r, Ok(true)),
        format!("pcall catches errors inside Lua: {r:?}"),
    );
    let r = lua
        .load("return string.format('%.3f %d %s', math.pi, 7, 'x')")
        .eval::<String>();
    check(
        "crate-mlua-format",
        r.as_deref().ok() == Some("3.142 7 x"),
        format!("string.format through newlib's printf: {r:?}"),
    );
    let before = lua.used_memory();
    let t = Instant::now();
    let r = lua
        .load("local t = {} for i = 1, 200000 do t[i] = tostring(i) .. 'x' end return #t")
        .eval::<i64>();
    let peak = lua.used_memory();
    let _ = lua.gc_collect();
    let _ = lua.gc_collect();
    check(
        "crate-mlua-gc",
        matches!(r, Ok(200000)),
        format!(
            "200k strings in {} ms; Lua heap {} KiB -> {} KiB -> {} KiB after GC",
            t.elapsed().as_millis(),
            before >> 10,
            peak >> 10,
            lua.used_memory() >> 10
        ),
    );
    let r = lua.load("return os.time()").eval::<i64>();
    check_warn(
        "crate-mlua-os-time",
        r.as_ref().is_ok_and(|t| *t > 1_600_000_000),
        format!("os.time() = {r:?}"),
    );
    let r = lua.load("local f = io.open('romfs:/captest.txt', 'r') if not f then return 'open failed' end local s = f:read('a') f:close() return s").eval::<String>();
    check_warn(
        "crate-mlua-io",
        r.as_deref().ok() == Some("switch_captest romfs OK\n"),
        format!("Lua io.open on romfs: {r:?}"),
    );
    let lua2 = lua.clone();
    let r = std::thread::spawn(move || lua2.load("return add(1, 1)").eval::<i64>()).join();
    check(
        "crate-mlua-send",
        matches!(r, Ok(Ok(2))),
        "the `send` feature: the Lua state used from another thread",
    );
}

#[cfg(feature = "tokio")]
fn tokio_test() {
    let rt = match tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_time()
        .thread_name("captest-tokio")
        .build()
    {
        Ok(rt) => rt,
        Err(e) => return fail("crate-tokio", format!("building a 2-worker runtime: {e}")),
    };
    let t = Instant::now();
    let sum = rt.block_on(async {
        let handles: Vec<_> = (0..100u64)
            .map(|i| {
                tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    i
                })
            })
            .collect();
        let mut sum = 0;
        for h in handles {
            sum += h.await.unwrap_or(0);
        }
        sum
    });
    let ms = t.elapsed().as_millis();
    check(
        "crate-tokio-tasks",
        sum == 4950 && ms < 1000,
        format!("100 tasks that each sleep 50 ms finished in {ms} ms"),
    );
    let t = Instant::now();
    // Inside async blocks: tokio's sleep and timeout find the runtime when they are created, and
    // an argument to block_on is created before block_on enters the runtime.
    rt.block_on(async { tokio::time::sleep(Duration::from_millis(100)).await });
    let ms = t.elapsed().as_secs_f64() * 1e3;
    check_warn(
        "crate-tokio-timer",
        (99.0..150.0).contains(&ms),
        format!("sleep(100 ms) took {ms:.1} ms"),
    );
    let got = rt.block_on(async {
        let (tx, mut rx) = tokio::sync::mpsc::channel::<u32>(16);
        tokio::spawn(async move {
            for i in 0..10_000 {
                if tx.send(i).await.is_err() {
                    break;
                }
            }
        });
        let mut n = 0;
        while rx.recv().await.is_some() {
            n += 1;
        }
        n
    });
    check(
        "crate-tokio-mpsc",
        got == 10_000,
        format!("{got}/10000 messages through a bounded channel across workers"),
    );
    let r = rt.block_on(async { tokio::task::spawn_blocking(|| (nx::core_now(), 42)).await });
    check(
        "crate-tokio-blocking",
        matches!(r, Ok((_, 42))),
        format!("spawn_blocking ran on core {:?}", r.map(|(c, _)| c).ok()),
    );
    let timeout = rt.block_on(async {
        tokio::time::timeout(Duration::from_millis(50), std::future::pending::<()>()).await
    });
    check(
        "crate-tokio-timeout",
        timeout.is_err(),
        "tokio::time::timeout fires",
    );
    let t = Instant::now();
    rt.shutdown_timeout(Duration::from_secs(2));
    info(
        "crate-tokio-shutdown",
        format!(
            "runtime and its threads shut down in {} ms",
            t.elapsed().as_millis()
        ),
    );
}

#[cfg(feature = "symphonia")]
fn symphonia_test() {
    use symphonia::core::codecs::{
        CODEC_TYPE_ADPCM_IMA_WAV, CODEC_TYPE_ADPCM_MS, CODEC_TYPE_MP3, DecoderOptions,
    };
    use symphonia::core::errors::Error;
    use symphonia::core::formats::FormatOptions;
    use symphonia::core::io::MediaSourceStream;
    use symphonia::core::meta::MetadataOptions;
    use symphonia::core::probe::Hint;
    let bytes = match std::fs::read("romfs:/assets/captest.wav") {
        Ok(b) => b,
        Err(e) => return fail("crate-symphonia", format!("romfs:/assets/captest.wav: {e}")),
    };
    let decoded = (|| -> Result<(u64, u32, usize), Error> {
        let mss = MediaSourceStream::new(Box::new(Cursor::new(bytes)), Default::default());
        let mut hint = Hint::new();
        hint.with_extension("wav");
        let mut format = symphonia::default::get_probe()
            .format(
                &hint,
                mss,
                &FormatOptions::default(),
                &MetadataOptions::default(),
            )?
            .format;
        let track = format
            .default_track()
            .ok_or(Error::Unsupported("no track"))?;
        let (id, rate) = (track.id, track.codec_params.sample_rate.unwrap_or(0));
        let mut decoder = symphonia::default::get_codecs()
            .make(&track.codec_params, &DecoderOptions::default())?;
        let (mut frames, mut packets) = (0u64, 0usize);
        loop {
            match format.next_packet() {
                Ok(p) if p.track_id() == id => {
                    frames += decoder.decode(&p)?.frames() as u64;
                    packets += 1;
                }
                Ok(_) => {}
                Err(Error::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                    return Ok((frames, rate, packets));
                }
                Err(e) => return Err(e),
            }
        }
    })();
    match decoded {
        Ok((frames, rate, packets)) => {
            check(
                "crate-symphonia-wav",
                frames == 28800 && rate == 48000,
                format!(
                    "decoded {frames} frames at {rate} Hz in {packets} packets (expected 28800 at 48000)"
                ),
            );
        }
        Err(e) => fail("crate-symphonia-wav", format!("decoding captest.wav: {e}")),
    }
    let codecs = symphonia::default::get_codecs();
    let has = |c| codecs.get_codec(c).is_some();
    check(
        "crate-symphonia-codecs",
        has(CODEC_TYPE_MP3) && has(CODEC_TYPE_ADPCM_MS) && has(CODEC_TYPE_ADPCM_IMA_WAV),
        "MP3 and ADPCM decoders are registered",
    );
}

#[cfg(feature = "bcdec")]
fn bcdec_test() {
    // One BC1 block: color0 = 0xF800 (pure red in RGB565), color1 = 0, every index 0.
    let block = [0x00, 0xF8, 0x00, 0x00, 0, 0, 0, 0];
    let mut out = [0u8; 4 * 4 * 4];
    bcdec_rs::bc1(&block, &mut out, 16);
    let all_red = out.chunks(4).all(|p| p == [255, 0, 0, 255]);
    check(
        "crate-bcdec-bc1",
        all_red,
        format!("BC1 block decodes to red; first pixel {:?}", &out[..4]),
    );
    let blocks = vec![0x55u8; 8 * 64 * 64];
    let mut img = vec![0u8; 256 * 256 * 4];
    let t = Instant::now();
    for (i, b) in blocks.chunks(8).enumerate() {
        let (bx, by) = (i % 64, i / 64);
        bcdec_rs::bc1(b, &mut img[(by * 4 * 256 + bx * 4) * 4..], 256 * 4);
    }
    info(
        "crate-bcdec-speed",
        format!(
            "256x256 BC1 texture decoded on the CPU in {:.2} ms",
            t.elapsed().as_secs_f64() * 1e3
        ),
    );
}

#[cfg(feature = "image")]
fn image_test() {
    match std::fs::read("romfs:/assets/captest.png").map(|b| image::load_from_memory(&b)) {
        Ok(Ok(img)) => {
            let px = img.to_rgba8().get_pixel(10, 20).0;
            check(
                "crate-image-png",
                img.width() == 64 && img.height() == 64 && px == [40, 80, 128, 255],
                format!(
                    "PNG {}x{}, pixel (10,20) = {px:?} (expected [40, 80, 128, 255])",
                    img.width(),
                    img.height()
                ),
            );
        }
        Ok(Err(e)) => fail("crate-image-png", e.to_string()),
        Err(e) => fail("crate-image-png", format!("romfs:/assets/captest.png: {e}")),
    }
    match std::fs::read("romfs:/assets/captest.jpg").map(|b| image::load_from_memory(&b)) {
        Ok(Ok(img)) => {
            check(
                "crate-image-jpeg",
                img.width() == 256 && img.height() == 256,
                format!("JPEG decoded, {}x{}", img.width(), img.height()),
            );
        }
        Ok(Err(e)) => fail("crate-image-jpeg", e.to_string()),
        Err(e) => fail(
            "crate-image-jpeg",
            format!("romfs:/assets/captest.jpg: {e}"),
        ),
    };
}

/// Bevy's three task pools, after TaskPoolPlugin has built them.
pub fn bevy_pools() {
    use bevy::tasks::{AsyncComputeTaskPool, ComputeTaskPool, IoTaskPool, block_on};
    report::heading("Bevy task pools");
    report::section("bevy-task-pools", 30, || {
        let compute = ComputeTaskPool::get();
        let (c, io, ac) = (
            compute.thread_num(),
            IoTaskPool::get().thread_num(),
            AsyncComputeTaskPool::get().thread_num(),
        );
        // Bevy's default TaskPoolOptions: IO and async compute each get 25% of the cores, rounded,
        // at least 1; compute gets what is left, at least 1. Up to 5 cores that is 1, 1, cores - 2.
        let cores = nx::info(0, 0).map_or(0, |m| m.count_ones() as usize);
        let expected = (cores.saturating_sub(2).max(1), 1, 1);
        let sized = format!(
            "compute {c} threads, IO {io}, async compute {ac}, from Bevy's default TaskPoolOptions and {} cores (the process core mask has {cores})",
            bevy::tasks::available_parallelism()
        );
        if (c, io, ac) != expected {
            fail(
                "bevy-pool-sizes",
                format!("{sized}; expected compute {}", expected.0),
            );
        } else {
            // The multi-threaded executor runs systems on compute threads only; the main thread waits.
            check_warn(
                "bevy-pool-sizes",
                c >= 2,
                if c >= 2 {
                    sized
                } else {
                    format!(
                        "{sized}: systems run one at a time; a game can raise TaskPoolOptions' compute.min_threads"
                    )
                },
            );
        }
        let steps = calibrate(50);
        let t = Instant::now();
        for _ in 0..4 {
            spin(steps);
        }
        let serial = t.elapsed().as_secs_f64();
        let t = Instant::now();
        let cores = compute.scope(|s| {
            for _ in 0..4 {
                s.spawn(async move {
                    spin(steps);
                    nx::core_now()
                });
            }
        });
        let parallel = t.elapsed().as_secs_f64();
        let mask = cores.iter().fold(0u32, |m, c| m | 1 << c);
        let speedup = serial / parallel.max(1e-6);
        check_warn(
            "bevy-compute-scope",
            cores.len() == 4 && (c < 2 || speedup > 1.4),
            format!(
                "4 x 50 ms of work in a scope: {speedup:.2}x faster than serial, ran on cores {mask:#05b}"
            ),
        );
        let r = block_on(
            IoTaskPool::get().spawn(async { std::fs::read_to_string("romfs:/captest.txt") }),
        );
        check(
            "bevy-io-pool",
            r.as_deref().ok() == Some("switch_captest romfs OK\n"),
            "IoTaskPool task read a romfs file",
        );
        let r = block_on(AsyncComputeTaskPool::get().spawn(async { (nx::core_now(), spin(1000)) }));
        info(
            "bevy-async-compute-pool",
            format!("AsyncComputeTaskPool task ran on core {}", r.0),
        );
        let tasks: Vec<_> = (0..64u32)
            .map(|i| AsyncComputeTaskPool::get().spawn(async move { i }))
            .collect();
        let sum: u32 = tasks.into_iter().map(block_on).sum();
        check(
            "bevy-many-tasks",
            sum == 2016,
            format!("64 small tasks returned {sum} (expected 2016)"),
        );
    });
}
