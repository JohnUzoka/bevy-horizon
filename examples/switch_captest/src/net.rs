//! std::net over libnx's BSD sockets (bevy_horizon initialises them when HorizonPlugin is added).

use std::io::{ErrorKind, Read, Write};
use std::net::{Ipv4Addr, Shutdown, SocketAddr, TcpListener, TcpStream, ToSocketAddrs, UdpSocket};
use std::os::fd::AsRawFd;
use std::thread;
use std::time::{Duration, Instant};

use crate::nx;
use crate::report::{self, check, check_warn, fail, info, pass, warn};

pub fn network() {
    report::heading("Network");
    report::section("net-tcp-loopback", 60, tcp_loopback);
    report::section("net-udp-loopback", 30, udp_loopback);
    report::section("net-socket-options", 30, socket_options);
    report::section("net-limits", 60, limits);
    report::section("net-internet", 90, internet);
}

/// Accepts one connection, giving up after `secs` (a blocking accept would hang the test if the
/// client never arrives).
fn accept_within(listener: &TcpListener, secs: u64) -> std::io::Result<(TcpStream, SocketAddr)> {
    listener.set_nonblocking(true)?;
    let deadline = Instant::now() + Duration::from_secs(secs);
    loop {
        match listener.accept() {
            Ok((s, peer)) => {
                // FreeBSD-derived stacks pass O_NONBLOCK on to accepted sockets.
                s.set_nonblocking(false)?;
                return Ok((s, peer));
            }
            Err(e) if e.kind() == ErrorKind::WouldBlock && Instant::now() < deadline => {
                thread::sleep(Duration::from_millis(5))
            }
            Err(e) => return Err(e),
        }
    }
}

fn tcp_loopback() {
    let listener = match TcpListener::bind("127.0.0.1:0") {
        Ok(l) => l,
        Err(e) => return fail("net-tcp-bind", format!("bind 127.0.0.1:0: {e}")),
    };
    let Ok(addr) = listener.local_addr() else {
        return fail("net-tcp-bind", "local_addr failed");
    };
    check(
        "net-tcp-bind",
        addr.port() != 0,
        format!("listening on {addr}"),
    );
    let server = thread::spawn(move || -> std::io::Result<(SocketAddr, usize)> {
        let (mut s, peer) = accept_within(&listener, 10)?;
        s.set_read_timeout(Some(Duration::from_secs(10)))?;
        let mut buf = vec![0u8; 16384];
        let mut total = 0;
        loop {
            let n = s.read(&mut buf)?;
            if n == 0 {
                return Ok((peer, total));
            }
            s.write_all(&buf[..n])?;
            total += n;
        }
    });
    let client = (|| -> std::io::Result<(SocketAddr, f64, bool)> {
        let mut c = TcpStream::connect_timeout(&addr, Duration::from_secs(5))?;
        c.set_read_timeout(Some(Duration::from_secs(10)))?;
        c.set_write_timeout(Some(Duration::from_secs(10)))?;
        let local = c.local_addr()?;
        let data: Vec<u8> = (0..256 * 1024).map(|i| (i * 31 % 251) as u8).collect();
        let mut back = vec![0u8; 8192];
        let mut same = true;
        let t = Instant::now();
        for chunk in data.chunks(8192) {
            c.write_all(chunk)?;
            c.read_exact(&mut back[..chunk.len()])?;
            same &= back[..chunk.len()] == *chunk;
        }
        let mbs = data.len() as f64 / t.elapsed().as_secs_f64() / 1e6;
        c.shutdown(Shutdown::Write)?;
        Ok((local, mbs, same))
    })();
    match (client, server.join()) {
        (Ok((local, mbs, same)), Ok(Ok((peer, total)))) => {
            check(
                "net-tcp-echo",
                same && total == 256 * 1024,
                format!("256 KiB echoed over loopback in 8 KiB chunks, {mbs:.1} MB/s"),
            );
            check(
                "net-tcp-peer-addr",
                peer == local,
                format!("server saw {peer}, client is {local}"),
            );
        }
        (Err(e), _) => fail("net-tcp-echo", format!("client: {e}")),
        (_, Ok(Err(e))) => fail("net-tcp-echo", format!("server: {e}")),
        (_, Err(_)) => fail("net-tcp-echo", "server thread panicked"),
    }
}

fn udp_loopback() {
    let (a, b) = match (
        UdpSocket::bind("127.0.0.1:0"),
        UdpSocket::bind("127.0.0.1:0"),
    ) {
        (Ok(a), Ok(b)) => (a, b),
        (Err(e), _) | (_, Err(e)) => return fail("net-udp", format!("bind: {e}")),
    };
    let (Ok(a_addr), Ok(b_addr)) = (a.local_addr(), b.local_addr()) else {
        return fail("net-udp", "local_addr failed");
    };
    let _ = b.set_read_timeout(Some(Duration::from_secs(2)));
    let mut buf = [0u8; 2048];
    let mut ok = 0;
    let t = Instant::now();
    for i in 0..200usize {
        let msg: Vec<u8> = (0..1200).map(|j| (i + j) as u8).collect();
        if a.send_to(&msg, b_addr).is_err() {
            continue;
        }
        if let Ok((n, from)) = b.recv_from(&mut buf) {
            ok += (n == 1200 && buf[..n] == msg[..] && from == a_addr) as u32;
        }
    }
    check(
        "net-udp",
        ok == 200,
        format!(
            "{ok}/200 1200-byte datagrams arrived intact in {:.0} ms",
            t.elapsed().as_secs_f64() * 1e3
        ),
    );

    let mut burst = |label: &str| {
        for i in 0..100u32 {
            let _ = a.send_to(&[i as u8; 1200], b_addr);
        }
        let _ = b.set_read_timeout(Some(Duration::from_millis(300)));
        let mut got = 0;
        while b.recv_from(&mut buf).is_ok() {
            got += 1;
        }
        info(
            "net-udp-burst",
            format!("{label}: sent 100 x 1200 B without reading, {got} arrived"),
        );
        got
    };
    let before = burst("default receive buffer");
    let fd = b.as_raw_fd();
    let want: libc::c_int = 256 * 1024;
    let set = unsafe {
        libc::setsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_RCVBUF,
            (&raw const want).cast(),
            size_of::<libc::c_int>() as libc::socklen_t,
        )
    };
    let mut have: libc::c_int = 0;
    let mut len = size_of::<libc::c_int>() as libc::socklen_t;
    let get = unsafe {
        libc::getsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_RCVBUF,
            (&raw mut have).cast(),
            &mut len,
        )
    };
    let after = burst("SO_RCVBUF 256 KiB");
    check_warn(
        "net-udp-rcvbuf",
        set == 0 && get == 0 && after > before,
        format!(
            "setsockopt(SO_RCVBUF, 256 KiB) = {set}, reads back {have} ({get}); burst {before} -> {after} delivered"
        ),
    );

    let _ = a.send_to(b"poll", b_addr);
    let mut pfd = libc::pollfd {
        fd,
        events: libc::POLLIN,
        revents: 0,
    };
    let n = unsafe { libc::poll(&mut pfd, 1, 1000) };
    check(
        "net-poll",
        n == 1 && pfd.revents & libc::POLLIN != 0,
        format!(
            "libc::poll on a UDP socket with data waiting: {n}, revents {:#x}",
            pfd.revents
        ),
    );
    let _ = b.recv_from(&mut buf);
    let connected = a
        .connect(b_addr)
        .and_then(|()| a.send(b"connected"))
        .and_then(|_| b.recv_from(&mut buf));
    check(
        "net-udp-connect",
        matches!(connected, Ok((9, _))),
        format!("connected UDP send: {connected:?}"),
    );
}

fn socket_options() {
    let listener = match TcpListener::bind("127.0.0.1:0") {
        Ok(l) => l,
        Err(e) => return fail("net-options", format!("bind: {e}")),
    };
    let _ = listener.set_nonblocking(true);
    match listener.accept() {
        Err(e) if e.kind() == ErrorKind::WouldBlock => pass(
            "net-nonblocking-accept",
            "accept with no client waiting returns WouldBlock",
        ),
        other => fail(
            "net-nonblocking-accept",
            format!("{:?}", other.map(|(_, a)| a)),
        ),
    }
    let Ok(addr) = listener.local_addr() else {
        return;
    };
    // Writes 4 bytes 3 s after accepting, so a read whose timeout is ignored still returns.
    let server = thread::spawn(move || -> std::io::Result<()> {
        let (mut s, _) = accept_within(&listener, 10)?;
        thread::sleep(Duration::from_secs(3));
        s.write_all(b"late")
    });
    let client = match TcpStream::connect_timeout(&addr, Duration::from_secs(5)) {
        Ok(c) => c,
        Err(e) => return fail("net-options", format!("connect: {e}")),
    };
    let nodelay = client.set_nodelay(true).and_then(|()| client.nodelay());
    check(
        "net-nodelay",
        matches!(nodelay, Ok(true)),
        format!("TCP_NODELAY: {nodelay:?}"),
    );
    let ttl = client.set_ttl(64).and_then(|()| client.ttl());
    check_warn("net-ttl", matches!(ttl, Ok(64)), format!("IP_TTL: {ttl:?}"));
    let mut b = [0u8; 4];
    let _ = client.set_nonblocking(true);
    let r = (&client).read(&mut b);
    check(
        "net-nonblocking-read",
        matches!(&r, Err(e) if e.kind() == ErrorKind::WouldBlock),
        format!(
            "nonblocking read with no data: {:?}",
            r.map_err(|e| e.kind())
        ),
    );
    let _ = client.set_nonblocking(false);
    let _ = client.set_read_timeout(Some(Duration::from_millis(200)));
    let t = Instant::now();
    let r = (&client).read(&mut b);
    let ms = t.elapsed().as_millis();
    check(
        "net-read-timeout",
        matches!(&r, Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut))
            && ms < 1500,
        format!(
            "200 ms read timeout: {:?} after {ms} ms (3000 ms means the timeout was ignored)",
            r.map_err(|e| e.kind())
        ),
    );
    let _ = client.set_read_timeout(Some(Duration::from_secs(5)));
    let late = (&client).read_exact(&mut b);
    let _ = server.join();
    check(
        "net-late-data",
        late.is_ok() && &b == b"late",
        format!("data sent 3 s later still arrives: {late:?}"),
    );
    match UdpSocket::bind("0.0.0.0:0") {
        Ok(u) => {
            let r = u.set_broadcast(true).and_then(|()| u.broadcast());
            check_warn(
                "net-broadcast",
                matches!(r, Ok(true)),
                format!("SO_BROADCAST: {r:?}"),
            );
        }
        Err(e) => warn("net-broadcast", format!("bind 0.0.0.0:0: {e}")),
    }
}

fn limits() {
    let mut socks = Vec::new();
    let mut err = None;
    for _ in 0..64 {
        match UdpSocket::bind("127.0.0.1:0") {
            Ok(s) => socks.push(s),
            Err(e) => {
                err = Some(e);
                break;
            }
        }
    }
    // libnx's default SocketInitConfig gives the socket service 2.2 MiB of buffers, and each UDP
    // socket reserves 50.25 KiB of it, so the Switch ran out at 42. A game that needs more
    // sockets than that would pass libnx a bigger config.
    let fits = socks.len() >= 32;
    let detail = format!(
        "{} UDP sockets open at once{}",
        socks.len(),
        err.map(|e| format!(", then: {e}")).unwrap_or_default()
    );
    if socks.len() == 64 {
        check("net-many-sockets", true, detail);
    } else if fits {
        info(
            "net-many-sockets",
            format!("{detail}; Horizon behaviour: libnx's default 2.2 MiB of socket buffers is the limit"),
        );
    } else {
        warn("net-many-sockets", detail);
    }
    drop(socks);
    match UdpSocket::bind("[::1]:0") {
        Ok(_) => info("net-ipv6", "an IPv6 loopback socket binds"),
        Err(e) => info("net-ipv6", format!("no IPv6 ({e}); bind IPv4 addresses")),
    }
    match ("localhost", 80).to_socket_addrs() {
        Ok(addrs) => {
            let v: Vec<_> = addrs.collect();
            check_warn(
                "net-dns-localhost",
                !v.is_empty(),
                format!("localhost resolves to {v:?}"),
            );
        }
        Err(e) => warn(
            "net-dns-localhost",
            format!("resolving localhost fails: {e}"),
        ),
    }
    let closed: SocketAddr = (Ipv4Addr::LOCALHOST, 1).into();
    match TcpStream::connect_timeout(&closed, Duration::from_secs(3)) {
        Ok(_) => warn("net-refused", "connecting to closed port 1 succeeded"),
        Err(e) => {
            check_warn(
                "net-refused",
                e.kind() == ErrorKind::ConnectionRefused,
                format!("connect to a closed port: {:?} ({e})", e.kind()),
            );
        }
    }
}

fn internet() {
    let rc = unsafe { nx::nifmInitialize(0) };
    if rc != 0 {
        return warn(
            "net-nifm",
            format!("nifmInitialize failed: {}", nx::rc_text(rc)),
        );
    }
    let (mut kind, mut strength, mut status) = (0u32, 0u32, 0u32);
    let rc = unsafe { nx::nifmGetInternetConnectionStatus(&mut kind, &mut strength, &mut status) };
    let mut ip = 0u32;
    let rc_ip = unsafe { nx::nifmGetCurrentIpAddress(&mut ip) };
    unsafe { nx::nifmExit() };
    if rc != 0 {
        info(
            "net-status",
            format!("no internet connection ({})", nx::rc_text(rc)),
        );
    } else {
        let kind = match kind {
            1 => "Wi-Fi",
            2 => "Ethernet",
            _ => "unknown",
        };
        info(
            "net-status",
            format!("{kind}, signal {strength}/3, status {status} (4 = connected)"),
        );
    }
    if rc_ip == 0 {
        info(
            "net-ip",
            format!("IP address {}", Ipv4Addr::from(ip.to_le_bytes())),
        );
    }
    if rc != 0 || status != 4 {
        return report::skip("net-internet", "offline, so DNS and HTTP were skipped");
    }
    let t = Instant::now();
    let addr = match ("example.com", 80).to_socket_addrs().map(|mut a| a.next()) {
        Ok(Some(a)) => {
            pass(
                "net-dns",
                format!("example.com -> {a} in {} ms", t.elapsed().as_millis()),
            );
            a
        }
        Ok(None) => return fail("net-dns", "example.com resolved to nothing"),
        Err(e) => return fail("net-dns", format!("resolving example.com: {e}")),
    };
    let t = Instant::now();
    let head = (|| -> std::io::Result<String> {
        let mut s = TcpStream::connect_timeout(&addr, Duration::from_secs(5))?;
        s.set_read_timeout(Some(Duration::from_secs(5)))?;
        s.write_all(b"HEAD / HTTP/1.0\r\nHost: example.com\r\nConnection: close\r\n\r\n")?;
        let mut buf = [0u8; 256];
        let n = s.read(&mut buf)?;
        Ok(String::from_utf8_lossy(&buf[..n])
            .lines()
            .next()
            .unwrap_or("")
            .to_string())
    })();
    match head {
        Ok(line) => {
            check(
                "net-http",
                line.starts_with("HTTP/1."),
                format!(
                    "HEAD http://example.com/: {line:?} in {} ms",
                    t.elapsed().as_millis()
                ),
            );
        }
        Err(e) => fail("net-http", format!("HEAD http://example.com/: {e}")),
    }
}
