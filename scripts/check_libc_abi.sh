#!/usr/bin/env bash
# Checks that patches/libc describes devkitA64's newlib and libnx headers exactly: type sizes and
# alignments, struct field offsets and constant values. Rust calls newlib/libnx through libc's
# definitions, and nothing else compares the two. A mismatch compiles, links and runs, and only
# shows up as wrong values at run time. Run it after any change to patches/libc.
set -euo pipefail

IMAGE_NAME="bevy-horizon-toolchain:local"
# Docker or Podman: CONTAINER_ENGINE picks one, otherwise docker if it's installed.
ENGINE="${CONTAINER_ENGINE:-$(command -v docker >/dev/null && echo docker || echo podman)}"
WORKSPACE_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

"$ENGINE" run --rm --log-driver=none -i \
    -v "$WORKSPACE_DIR:/workspace:z" \
    -w /workspace \
    "$IMAGE_NAME" \
    python3 - <<'PYIN'
import os, re, shutil, struct, subprocess, sys

WORK = "/workspace/target/libc-abi-check-work"
TARGET_DIR = "/workspace/target/libc-abi-check"
BIN = "/opt/devkitpro/devkitA64/bin/aarch64-none-elf-"
SENTINEL = 0xDEADBEEF

TYPES = ["long", "time_t", "clockid_t", "off_t", "dev_t", "ino_t", "mode_t", "nlink_t", "uid_t",
         "gid_t", "pid_t", "ssize_t", "blksize_t", "blkcnt_t", "suseconds_t", "clock_t",
         "pthread_t", "pthread_key_t", "pthread_attr_t", "pthread_mutex_t", "pthread_mutexattr_t",
         "pthread_cond_t", "pthread_condattr_t", "pthread_rwlock_t", "pthread_rwlockattr_t",
         "sa_family_t", "socklen_t", "in_addr_t", "in_port_t", "nfds_t"]
OPAQUE = {"pthread_attr_t", "pthread_mutex_t", "pthread_mutexattr_t", "pthread_cond_t",
          "pthread_condattr_t", "pthread_rwlock_t", "pthread_rwlockattr_t"}
STRUCTS = {
    "stat": ["st_dev", "st_ino", "st_mode", "st_nlink", "st_uid", "st_gid", "st_rdev", "st_size",
             "st_atim", "st_mtim", "st_ctim", "st_blksize", "st_blocks", "st_spare4"],
    "timespec": ["tv_sec", "tv_nsec"],
    "timeval": ["tv_sec", "tv_usec"],
    "dirent": ["d_ino", "d_type", "d_name"],
    "sched_param": ["sched_priority"],
    "sockaddr": ["sa_len", "sa_family", "sa_data"],
    "sockaddr_in": ["sin_len", "sin_family", "sin_port", "sin_addr", "sin_zero"],
    "sockaddr_in6": ["sin6_family", "sin6_port", "sin6_flowinfo", "sin6_addr", "sin6_scope_id"],
    "sockaddr_storage": ["ss_len", "ss_family"],
    "in_addr": ["s_addr"],
    "in6_addr": ["s6_addr"],
    "ip_mreq": ["imr_multiaddr", "imr_interface"],
    "linger": ["l_onoff", "l_linger"],
    "pollfd": ["fd", "events", "revents"],
    "hostent": ["h_name", "h_aliases", "h_addrtype", "h_length", "h_addr_list"],
    "addrinfo": ["ai_flags", "ai_family", "ai_socktype", "ai_protocol", "ai_addrlen", "ai_canonname",
                 "ai_addr", "ai_next"],
    "fd_set": [],
    "iovec": ["iov_base", "iov_len"],
}
CONSTANTS = """EPERM ENOENT EINTR EIO EBADF EAGAIN ENOMEM EACCES EBUSY EEXIST EXDEV ENOTDIR EISDIR
    EINVAL ENOSPC EROFS EPIPE ERANGE ENAMETOOLONG ENOSYS ENOTEMPTY ETIMEDOUT EWOULDBLOCK ELOOP EFBIG
    EMFILE ENFILE ESPIPE EDEADLK ENOTSUP EOPNOTSUPP ETXTBSY EINPROGRESS EALREADY ENOTCONN
    ECONNREFUSED ECONNRESET EADDRINUSE EHOSTUNREACH ENETDOWN ENOBUFS EMLINK ESTALE EDQUOT ENOTSOCK
    EPROTO O_RDONLY O_WRONLY O_RDWR O_ACCMODE O_APPEND O_CREAT O_TRUNC O_EXCL O_CLOEXEC O_NONBLOCK
    O_SYNC S_IFMT S_IFDIR S_IFREG S_IFLNK S_IFIFO S_IFCHR S_IFBLK S_IFSOCK S_IRUSR S_IWUSR
    DT_UNKNOWN DT_DIR DT_REG DT_LNK SEEK_SET SEEK_CUR SEEK_END CLOCK_REALTIME CLOCK_MONOTONIC
    PTHREAD_MUTEX_NORMAL PTHREAD_MUTEX_RECURSIVE F_GETFD F_SETFD F_GETFL F_SETFL FD_CLOEXEC
    _SC_PAGESIZE _SC_NPROCESSORS_ONLN
    AF_UNSPEC AF_UNIX AF_INET AF_INET6 PF_UNSPEC PF_INET PF_INET6 SOCK_STREAM SOCK_DGRAM SOCK_RAW
    SOCK_RDM SOCK_SEQPACKET SOCK_CLOEXEC SOCK_NONBLOCK SOL_SOCKET SO_DEBUG SO_REUSEADDR SO_KEEPALIVE
    SO_BROADCAST SO_LINGER SO_RCVBUF SO_SNDBUF SO_RCVTIMEO SO_SNDTIMEO SO_ERROR SO_TYPE IPPROTO_IP
    IPPROTO_TCP IPPROTO_UDP IPPROTO_IPV6 IP_TOS IP_TTL IP_MULTICAST_IF IP_MULTICAST_TTL
    IP_MULTICAST_LOOP IP_ADD_MEMBERSHIP IP_DROP_MEMBERSHIP IPV6_UNICAST_HOPS IPV6_MULTICAST_IF
    IPV6_MULTICAST_HOPS IPV6_MULTICAST_LOOP IPV6_V6ONLY IPV6_JOIN_GROUP IPV6_LEAVE_GROUP
    IPV6_ADD_MEMBERSHIP IPV6_DROP_MEMBERSHIP TCP_NODELAY TCP_MAXSEG TCP_NOPUSH TCP_NOOPT TCP_KEEPIDLE
    TCP_KEEPINTVL TCP_KEEPCNT MSG_OOB MSG_PEEK MSG_DONTROUTE MSG_EOR MSG_TRUNC MSG_CTRUNC MSG_WAITALL
    MSG_DONTWAIT MSG_NOSIGNAL MSG_MORE POLLIN POLLPRI POLLOUT POLLRDNORM POLLWRNORM POLLRDBAND
    POLLWRBAND POLLERR POLLHUP POLLNVAL SHUT_RD SHUT_WR SHUT_RDWR INADDR_ANY INADDR_BROADCAST
    INADDR_LOOPBACK INADDR_NONE EAI_AGAIN EAI_BADFLAGS EAI_FAIL EAI_FAMILY EAI_MEMORY EAI_NONAME
    EAI_SERVICE EAI_SOCKTYPE EAI_SYSTEM EAI_BADHINTS EAI_PROTOCOL EAI_OVERFLOW AI_PASSIVE
    AI_CANONNAME AI_NUMERICHOST AI_NUMERICSERV AI_ADDRCONFIG NI_MAXHOST NI_MAXSERV NI_NOFQDN
    NI_NUMERICHOST NI_NAMEREQD NI_NUMERICSERV NI_DGRAM FIONBIO FIONREAD""".split()
HEADERS = ["stddef.h", "sys/types.h", "sys/stat.h", "sys/time.h", "errno.h", "fcntl.h", "unistd.h",
           "time.h", "pthread.h", "dirent.h", "sched.h", "sys/socket.h", "netinet/in.h",
           "netinet/tcp.h", "arpa/inet.h", "poll.h", "netdb.h", "sys/select.h", "sys/ioctl.h",
           "sys/filio.h"]
MISSING_IN_C_OK = {
    "value MSG_MORE": "libnx has no MSG_MORE flag; libc keeps a zero compatibility no-op",
    "value IPV6_ADD_MEMBERSHIP": "libnx exposes IPV6_JOIN_GROUP instead; libc alias is a compatibility no-op name",
    "value IPV6_DROP_MEMBERSHIP": "libnx exposes IPV6_LEAVE_GROUP instead; libc alias is a compatibility no-op name",
}

rows = []
for t in TYPES:
    r = "c_long" if t == "long" else t
    rows.append((f"sizeof {t}", f"sizeof({t})", f"size_of::<libc::{r}>()", t in OPAQUE, False))
    rows.append((f"alignof {t}", f"_Alignof({t})", f"align_of::<libc::{r}>()", t in OPAQUE, False))
for s, fields in STRUCTS.items():
    rows.append((f"sizeof struct {s}", f"sizeof(struct {s})", f"size_of::<libc::{s}>()", False, False))
    rows.append((f"alignof struct {s}", f"_Alignof(struct {s})", f"align_of::<libc::{s}>()", False, False))
    rows += [(f"offsetof {s}.{f}", f"offsetof(struct {s}, {f})", f"offset_of!(libc::{s}, {f})", False, False)
             for f in fields]
rows += [(f"value {k}", f"(unsigned long)({k})", f"libc::{k} as u64", False, True) for k in CONSTANTS]

if os.path.exists(WORK):
    shutil.rmtree(WORK)
os.makedirs(f"{WORK}/src", exist_ok=True)
with open(f"{WORK}/probe.c", "w") as f:
    f.write("".join(f"#include <{h}>\n" for h in HEADERS))
    f.write("const unsigned long PROBE[] = {\n")
    for label, c, _, _, is_const in rows:
        if is_const:
            name = label.split(" ", 1)[1]
            f.write(f"#ifdef {name}\n    {c}, /* {label} */\n#else\n    {SENTINEL}UL, /* {label} missing in C */\n#endif\n")
        else:
            f.write(f"    {c}, /* {label} */\n")
    f.write("};\n")
with open(f"{WORK}/Cargo.toml", "w") as f:
    f.write('[package]\nname = "libc_abi_probe"\nversion = "0.0.0"\nedition = "2021"\n\n'
            '[dependencies]\nlibc = { path = "/workspace/patches/libc", default-features = false }\n\n'
            '[workspace]\n')
with open(f"{WORK}/src/lib.rs", "w") as f:
    f.write("#![no_std]\nuse core::mem::{align_of, offset_of, size_of};\n\n#[unsafe(no_mangle)]\n#[used]\n")
    f.write(f"pub static PROBE: [u64; {len(rows)}] = [\n")
    f.write("".join(f"    {r} as u64, // {label}\n" for label, _, r, _, _ in rows) + "];\n")

subprocess.run([BIN + "gcc", "-D__SWITCH__", "-I/opt/devkitpro/libnx/include", "-march=armv8-a+crc+crypto",
                "-mtune=cortex-a57", "-mtp=soft", "-fPIE", "-O2", "-S", f"{WORK}/probe.c",
                "-o", f"{WORK}/probe.s"], check=True)
c_values = []
for line in open(f"{WORK}/probe.s"):
    if m := re.match(r"\s*\.xword\s+(\S+)", line):
        c_values.append(int(m.group(1), 0) & ((1 << 64) - 1))
    elif m := re.match(r"\s*\.zero\s+(\d+)", line):
        c_values += [0] * (int(m.group(1)) // 8)

subprocess.run(["cargo", "rustc", "-q", "--release", "-Zbuild-std=core", "-Zjson-target-spec",
                "--target", "/workspace/aarch64-nintendo-horizon.json", "--lib", "--", "--emit=obj"],
               cwd=WORK, env=dict(os.environ, CARGO_TARGET_DIR=TARGET_DIR), check=True)
objects = []
for root, _, files in os.walk(TARGET_DIR):
    objects += [os.path.join(root, n) for n in files if n.startswith("libc_abi_probe-") and n.endswith(".o")]
obj = max(objects, key=os.path.getmtime)
symbols = subprocess.run([BIN + "objdump", "-t", obj], capture_output=True, text=True, check=True)
fields = next(l.split() for l in symbols.stdout.splitlines() if l.split()[-1:] == ["PROBE"])
offset, section = int(fields[0], 16), fields[-3]
subprocess.run([BIN + "objcopy", "-O", "binary", f"--only-section={section}", obj, f"{WORK}/probe.bin"], check=True)
data = open(f"{WORK}/probe.bin", "rb").read()[offset:offset + 8 * len(rows)]
rust_values = struct.unpack(f"<{len(rows)}Q", data)

if len(c_values) != len(rows):
    sys.exit(f"error: read {len(c_values)} values from the C side, expected {len(rows)}")
mismatches = bigger = missing_c = 0
missing_notes = []
print(f"{'':46}{'C':>22}{'Rust':>22}")
for (label, _, _, opaque, _), c, r in zip(rows, c_values, rust_values):
    note = ""
    if c == SENTINEL:
        if label in MISSING_IN_C_OK:
            note, missing_c = "  missing in C: " + MISSING_IN_C_OK[label], missing_c + 1
            missing_notes.append(f"{label}: {MISSING_IN_C_OK[label]}")
        else:
            note, mismatches = "  <-- MISSING IN C", mismatches + 1
    elif r != c:
        if opaque and r > c:
            note, bigger = "  bigger in Rust: safe, it is opaque", bigger + 1
        else:
            note, mismatches = "  <-- MISMATCH", mismatches + 1
    print(f"{label:46}{c:22}{r:22}{note}")
print(f"\n{len(rows)} items compared: {mismatches} mismatches, {missing_c} documented missing-in-C constants, "
      f"{bigger} opaque pthread sizes or alignments bigger in Rust (safe).")
if missing_notes:
    print("Missing in C and documented as harmless:")
    for note in missing_notes:
        print("  " + note)
sys.exit(1 if mismatches else 0)
PYIN
