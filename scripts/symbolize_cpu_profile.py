#!/usr/bin/env python3
"""Names the offsets in a bevy_horizon CPU profile (crates/bevy_horizon/src/profiler.rs).

Usage: scripts/symbolize_cpu_profile.py <cpu-profile.txt> <the game's ELF> [--top N] [--chains N]
                                        [--threads N] [--lines] [--width N]

The ELF is the one the NRO was made from (<game>/target/aarch64-nintendo-horizon/release/<bin>).
It is linked at 0, so the report's offsets are its addresses. Function names come from `nm` (GNU
binutils reads an AArch64 ELF on any host). --lines adds file:line for the hottest addresses with
`addr2line`, which takes a minute or two on a large game.

The approach follows UnleashedRecomp-NX's tools/switch-cpu-profile.py (github.com/ChanseyIsTheBest/
UnleashedRecomp-NX, branch switch-1.0.0; GPL-3.0): nm's symbol table searched by bisection, and
tables per thread and over all threads of where CPU time goes and where threads wait. This is a
separate implementation and copies none of its code.

What it prints:
- each thread: how busy it was (its CPU time over the time it was watched, from the kernel), how
  many samples found it running rather than waiting in a system call, and where it started (the
  outermost frame that isn't thread plumbing);
- over all threads, where CPU time goes: each thread's CPU time shared out over the functions its
  running samples were in;
- for the busiest threads, the functions they run in (self), the ones they run under
  (inclusive), and where they wait.
A running sample is a thread that wasn't in a system call. On a busy CPU it may have been ready
but not on a core, so a thread's busy % is the measure of its CPU time; its samples say where.
"""

import argparse
import bisect
import collections
import re
import subprocess
import sys

FLAG_WAITING, FLAG_LR, FLAG_SCANNED, FLAG_CAPPED = 1, 2, 4, 8
GAP, OUTSIDE = "-", "?"
# Rust's legacy mangling leaves a hash on each name.
HASH = re.compile(r"::h[0-9a-f]{16}$")
PATH = re.compile(r"\b[A-Za-z_]\w*(?:::[A-Za-z_]\w*){4,}")
NM_LINE = re.compile(r"^([0-9a-fA-F]+) (?:([0-9a-fA-F]{16}) )?([A-Za-z]) (.+)$")
# Frames between a thread's start and the code it was started to run.
PLUMBING = re.compile(
    r"^(std::(sys|thread|rt|panicking|panic)::|core::ops::function::|core::panic::|"
    r"<?alloc::boxed::Box<|<core::panic::unwind_safe::|std::sys::backtrace::|__rust_|"
    r"bevy_horizon::compat::|__wrap_|thread_start$|threadEntry$|_start$|__libnx_|"
    r"<F as core::ops::function|<std::thread::|core::ops::function)"
)


def parse(path):
    header, threads, thread = {}, [], None
    with open(path, encoding="utf-8", errors="replace") as report:
        for line in report:
            line = line.strip()
            if not line or line.startswith("#"):
                continue
            if line.startswith("thread "):
                parts = line.split()
                fields = dict(part.split("=", 1) for part in parts[2:] if "=" in part)
                thread = {"name": parts[1], "chains": []}
                for key in ("samples", "waiting"):
                    thread[key] = int(fields.get(key, 0))
                for key in ("cpu_ms", "seen_ms", "busy_pct"):
                    thread[key] = float(fields.get(key, 0))
                threads.append(thread)
            elif line.startswith("c "):
                if thread is None:
                    sys.exit(f"{path}: a chain before any thread line")
                parts = line.split()
                frames = [part if part in (GAP, OUTSIDE) else int(part, 16) for part in parts[3:]]
                thread["chains"].append((int(parts[1]), int(parts[2], 16), frames))
            elif line == "end":
                header["end"] = True
            else:
                key, _, value = line.partition(" ")
                header[key] = value
    if header.get("version") != "1":
        sys.exit(f"{path}: not a version 1 bevy_horizon CPU profile")
    if not header.get("end"):
        print(f"warning: {path} has no `end` line; it may have been cut short", file=sys.stderr)
    return header, threads


class Symbols:
    def __init__(self, elf):
        try:
            listing = subprocess.run(
                ["nm", "-n", "-S", "--defined-only", "-C", elf],
                capture_output=True, text=True, check=True,
            ).stdout
        except (OSError, subprocess.CalledProcessError) as error:
            sys.exit(f"nm could not read {elf}: {error}")
        symbols = []
        for line in listing.splitlines():
            match = NM_LINE.match(line)
            if match and match.group(3) in "TtWw":
                size = int(match.group(2), 16) if match.group(2) else 0
                symbols.append((int(match.group(1), 16), size, HASH.sub("", match.group(4)), match.group(3)))
        if not symbols:
            sys.exit(f"{elf} has no function symbols")
        # Of names for one address, the sized and global one.
        symbols.sort(key=lambda s: (s[0], s[1] == 0, s[3].islower()))
        self.starts, self.symbols = [], []
        for symbol in symbols:
            if not self.starts or self.starts[-1] != symbol[0]:
                self.starts.append(symbol[0])
                self.symbols.append(symbol)
        self.cache = {}

    def name(self, address):
        name = self.cache.get(address)
        if name is None:
            index = bisect.bisect_right(self.starts, address) - 1
            if index < 0:
                name = f"?{address:#x}"
            else:
                start, size, symbol, _ = self.symbols[index]
                name = symbol if not size or address < start + size else f"?{address:#x} (after {symbol})"
            self.cache[address] = name
        return name


def frame_names(symbols, flags, frames):
    """The chain's functions, leaf first, with a repeat of the same function kept once."""
    names = []
    for index, frame in enumerate(frames):
        if frame == GAP:
            name = "…"
        elif frame == OUTSIDE:
            name = "? (outside the NRO)"
        else:
            # A return address is just after its call, which may end the function it is in.
            name = symbols.name(frame if index == 0 else frame - 4)
        if not names or names[-1] != name:
            names.append(name)
    return names


def entry_of(names):
    for name in reversed(names):
        if name != "…" and not name.startswith("?") and not PLUMBING.match(name):
            return name
    return names[-1] if names else "?"


WIDTH = 0  # --width


def short(name, width=None):
    """`name`, cut in the middle to `width` (--width; 0 keeps it whole) so the method stays."""
    width = WIDTH if width is None else width
    if not width or len(name) <= width:
        return name
    tail = width // 3
    return name[: width - tail - 1] + "…" + name[-tail:]


def compact(name):
    """`name` without generic arguments, its long paths as crate::…::parent::item."""
    out, i = [], 0
    while i < len(name):
        if name[i] == "<" and i > 0 and (name[i - 1].isalnum() or name[i - 1] in "_:"):
            depth, j = 1, i + 1
            while j < len(name) and depth:
                if name[j] == "<":
                    depth += 1
                elif name[j] == ">" and name[j - 1] != "-":
                    depth -= 1
                j += 1
            if out[-2:] == [":", ":"]:
                del out[-2:]
            i = j
            continue
        out.append(name[i])
        i += 1
    shorten = lambda m: (lambda parts: parts[0] + "::…::" + "::".join(parts[-2:]))(m.group(0).split("::"))
    return PATH.sub(shorten, "".join(out))


def table(title, counts, total, top, unit="%"):
    print(f"  {title}:")
    if not total:
        print("    (none)")
        return
    for name, value in counts.most_common(top):
        if unit == "%":
            print(f"    {100.0 * value / total:5.1f}%  {short(name)}")
        else:
            print(f"    {value:8.1f} ms {100.0 * value / total:5.1f}%  {short(name)}")


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("report")
    parser.add_argument("elf")
    parser.add_argument("--top", type=int, default=15, help="rows per table (15)")
    parser.add_argument("--threads", type=int, default=6, help="threads to break down, busiest first (6)")
    parser.add_argument("--chains", type=int, default=0, help="also print each thread's N commonest chains")
    parser.add_argument("--lines", action="store_true", help="file:line of the hottest addresses (slow)")
    parser.add_argument("--width", type=int, default=0, help="cut names to this many characters (0: whole)")
    args = parser.parse_args()
    global WIDTH
    WIDTH = args.width

    header, threads = parse(args.report)
    symbols = Symbols(args.elf)

    window = float(header.get("window_ms", 0))
    sampler = float(header.get("sampler_ms", 0))
    print(f"{args.report}: {window / 1000:.1f} s watched, {header.get('passes')} passes "
          f"every {header.get('interval_us')} us; the sampler took {sampler:.0f} ms "
          f"({100 * sampler / window if window else 0:.1f}% of one core)")
    for key in ("pause_failures", "resume_failures", "exited_while_paused", "untracked_threads", "capped"):
        if header.get(key, "0") != "0":
            print(f"  note: {key} {header[key]}")

    named = []
    for thread in threads:
        running, waiting = collections.Counter(), collections.Counter()
        inclusive, entries = collections.Counter(), collections.Counter()
        hot_pcs = collections.Counter()
        for count, flags, frames in thread["chains"]:
            names = frame_names(symbols, flags, frames)
            entries[entry_of(names)] += count
            if flags & FLAG_WAITING:
                site = [compact(name) for name in names if name != "…"][:4]
                waiting[" < ".join(site)] += count
            else:
                running[names[0]] += count
                for name in set(names) - {"…"}:
                    inclusive[name] += count
                if isinstance(frames[0], int):
                    hot_pcs[frames[0]] += count
        named.append((thread, running, waiting, inclusive, entries, hot_pcs))

    # Reports from before the profiler asked with the 13.0.0 id have no kernel CPU times: no
    # `cpu_times` line and every cpu_ms 0. Estimate each thread's from its share of running samples
    # over the time it was watched, which counts a thread ready but waiting for a core as busy.
    if header.get("cpu_times", "none") == "none" and all(t["cpu_ms"] == 0 for t in threads):
        print("  note: no kernel CPU times in this report; busy % and cpu_ms below are estimated "
              "from running samples (an upper bound)")
        for thread, running, *_ in named:
            share = sum(running.values()) / thread["samples"] if thread["samples"] else 0
            thread["busy_pct"] = 100 * share
            thread["cpu_ms"] = thread["seen_ms"] * share

    print("\nThreads (busy: CPU time over the time watched; running: samples not in a system call):")
    print(f"  {'thread':>7} {'busy%':>6} {'cpu_ms':>8} {'samples':>8} {'running':>8}  started in")
    for thread, running, _, _, entries, _ in named:
        entry = compact(entries.most_common(1)[0][0]) if entries else "?"
        print(f"  {thread['name']:>7} {thread['busy_pct']:6.1f} {thread['cpu_ms']:8.0f} "
              f"{thread['samples']:8d} {sum(running.values()):8d}  {entry}")

    # Each thread's CPU time, shared out over where its running samples were.
    cpu = collections.Counter()
    for thread, running, *_ in named:
        total = sum(running.values())
        for name, count in running.items():
            cpu[name] += thread["cpu_ms"] * count / total if total else 0
    total_cpu = sum(thread["cpu_ms"] for thread, *_ in named)
    print(f"\nAll threads: {total_cpu:.0f} ms of CPU time over {window / 1000:.1f} s "
          f"({total_cpu / window if window else 0:.2f} cores busy of 3)")
    table("where it goes (self, estimated)", cpu, total_cpu, args.top, unit="ms")

    hot_pcs_all = collections.Counter()
    for thread, running, waiting, inclusive, entries, hot_pcs in sorted(
        named, key=lambda item: item[0]["cpu_ms"], reverse=True
    )[: args.threads]:
        hot_pcs_all.update(hot_pcs)
        total_running = sum(running.values())
        entry = compact(entries.most_common(1)[0][0]) if entries else "?"
        print(f"\n== {thread['name']}: busy {thread['busy_pct']:.1f}%, {thread['cpu_ms']:.0f} ms CPU, "
              f"{total_running} of {thread['samples']} samples running; started in {entry}")
        table("running in (self)", running, total_running, args.top)
        table("running under (inclusive)", inclusive, total_running, args.top)
        table("waiting at", waiting, thread["samples"], min(args.top, 8))
        if args.chains:
            print("  commonest chains (leaf first):")
            chains = sorted(thread["chains"], key=lambda chain: chain[0], reverse=True)[: args.chains]
            for count, flags, frames in chains:
                kind = "wait" if flags & FLAG_WAITING else "run"
                scanned = " [stack scanned]" if flags & FLAG_SCANNED else ""
                print(f"    {count:6d} {kind}{scanned}")
                for name in frame_names(symbols, flags, frames):
                    print(f"           {short(name)}")

    if args.lines and hot_pcs_all:
        addresses = [pc for pc, _ in hot_pcs_all.most_common(args.top * 2)]
        print("\nHottest addresses (running samples, busiest threads):", flush=True)
        try:
            out = subprocess.run(
                ["addr2line", "-e", args.elf, "-f", "-C", "-a"] + [f"{a:#x}" for a in addresses],
                capture_output=True, text=True, check=True,
            ).stdout.splitlines()
        except (OSError, subprocess.CalledProcessError) as error:
            sys.exit(f"addr2line failed: {error}")
        # -a -f: an address line, a function line and a file:line line for each address.
        for index, address in enumerate(addresses):
            where = out[3 * index + 2] if 3 * index + 2 < len(out) else "?"
            print(f"  {hot_pcs_all[address]:6d} {address:#10x} {where}  {short(compact(symbols.name(address)), 90)}")


if __name__ == "__main__":
    main()
