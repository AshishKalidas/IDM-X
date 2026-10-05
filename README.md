# IDMX

**Download faster by using every network you have at once.**
Wi-Fi, Ethernet, a USB-tethered phone, a cellular modem — IDMX binds each connection to a specific
network and pulls different parts of the *same file* over all of them simultaneously.

A Rust engine, a Tauri desktop app with an Apple-style UI (true-black OLED dark mode, true-white
light mode), a CLI, a local HTTP API with a web dashboard, and a browser extension — one engine
underneath all of them.

```
                ┌── Wi-Fi ──────────┐
file ─ split ───┼── iPhone USB ─────┼──▶ one file on disk
                └── Cellular ───────┘
```

## Why I built this

Years ago I had a Samsung Galaxy Note 4, which shipped with a **Download Booster**: it combined
Wi-Fi and the cellular connection to download a file faster than either could alone. I loved that
feature, and I was surprised how rarely it showed up anywhere else.

IDMX started from that memory. I wanted the same idea on a laptop — any number of networks, not
just two — and I wanted it to live inside a proper **download manager**: a queue, resume, scheduling,
rules, a CLI and a browser extension, instead of a single toggle in a settings menu.

## The problem

Most people have more internet than any single download uses. A laptop on Wi-Fi, a phone with a data
plan, maybe an Ethernet port — yet a download opens every connection on whichever network the operating
system prefers and leaves the rest idle. Servers often throttle each connection, so one socket on one
network rarely reaches what the line can do, and when that network drops the transfer starts over.

IDMX treats the networks as a pool. It splits a file into blocks, ties each connection to a *specific*
network at the socket level, and lets whichever network is free take the next block — so a fast link
carries more and a slow one less, without anyone tuning it. The result is verified, resumable, and
reassembled on disk as one file.

## Why this stack

The engine is the product, and what it does is unusual enough that the stack followed from it.

* **Rust for the engine.** The hard part is pinning traffic to an interface (`IP_BOUND_IF` on macOS,
  `SO_BINDTODEVICE` on Linux) and running dozens of concurrent, cancellable transfers with shared
  state — a block table, rate limiters, retry and health tracking. Rust gives that without a garbage
  collector's pauses and without data races: the compiler rejects the class of concurrency bugs that a
  scheduler like this is most prone to. It also compiles to one native binary, which is why the CLI
  needs no runtime.
* **One engine, several front doors.** The desktop app, the `idmx` CLI, the browser extension and the web
  dashboard all talk to the same local HTTP/SSE API (token-authenticated, loopback by default). No
  feature is built twice, and anything the app can do can be scripted.
* **Tauri instead of Electron.** The UI runs in the system webview and the engine runs in-process, so a
  full desktop build is about 11 MB rather than a bundled copy of Chromium, and there is no second
  runtime to keep in sync with the engine.
* **React + TypeScript for the interface.** The UI is mostly live data — throughput, per-connection
  state, a block map — so a typed component model with a canvas layer for the visuals fits better than
  anything native-per-platform, and it is the same code on macOS and Windows.
* **Proven libraries where the problem is already solved.** BitTorrent is `librqbit`; video extraction is
  `yt-dlp`, run as a managed, checksum-verified helper rather than reimplemented. The multi-network
  scheduling is the part written from scratch.
* **Tested against a hostile server.** The test suite runs the engine against a local origin that injects
  faults — dropped connections, slow sockets, changed files, missing range support — rather than
  trusting the happy path.

## What's in the box

| | |
|---|---|
| `crates/core` | The engine: probing, block scheduler, work-stealing, tail hedging, resume, health, rate limits, queue, post-processing |
| `crates/server` | HTTP/SSE API + embedded web UI (token auth, CORS locked to extensions) |
| `crates/cli` | `idmx` — `get`, `add`, `ls`, `daemon`, `nets`, `net`, `connections`, `speedtest`, `update`, … |
| `src-tauri` | Desktop app: tray with live speed, notifications, graceful quit |
| `ui` | React + canvas UI (constellation, throughput chart, chunk map) |
| `extension` | Chromium (Chrome, Edge, Brave…) MV3 extension: finds media, catches magnet links, pairs itself with the app |
| `website` | The project site: features, downloads, release notes |

## Run it

```bash
npm install                 # once (Tauri CLI)
npm --prefix ui install
npm run ui                  # build the web UI
cargo run -p idmx-app       # the desktop app
cargo run -p idmx-cli -- --help
./scripts/mac-app.sh --open  # macOS: build and open IDMX.app
npm run app:build           # installers: .app / .dmg / .msi / .deb
```

Requires Rust 1.80+ and Node 20+. On Linux install `webkit2gtk` for the app.

## Using a phone's data

The Networks page has an **Add your phone's mobile data** guide with live status. iPhone: plug in, turn on
Personal Hotspot → *Allow Others to Join*. Android: turn on USB tethering; on a Mac install the free
[TetherKit](https://github.com/XiaoMiku01/TetherKit/releases/download/v0.1.5/TetherKit-v0.1.5-macos-arm64.zip) helper first (direct download, Apple-silicon build) (macOS can't share an Android phone's data on its own).

## Command line

IDMX has a full terminal interface, `idmx`, that works with or without the app open.

```bash
./scripts/install-cli.sh        # builds it and copies it to /usr/local/bin
idmx nets                       # your networks, with live ping
idmx get <url>                  # download using every network
idmx get "magnet:?xt=…"         # torrents
idmx get --quality 720 <video>  # video (audio, audio216, audio320 for MP3)
idmx info <link>                # look inside a torrent / video first
idmx daemon                     # headless engine + web dashboard
idmx --help                     # everything else (ls, pause, resume, net, speedtest…)
```

If the app is running, `idmx ls`, `pause`, `resume` and the rest control it.

## Video, torrents and the browser

* **First run** walks you through installing the browser extension and yt-dlp.
* **Extension**: sniffs media on the page (badge count + popup), adds a download chip over videos, takes
  over magnet links and `.torrent` downloads, and sends your cookies/referer so logged-in downloads work.
* **Video**: paste a YouTube (etc.) link, pick a quality. yt-dlp, Deno (for YouTube) and ffmpeg install
  into IDMX's own folder with checksum verification.
* **Torrents**: paste a magnet or `.torrent`, choose files, seed to a ratio. `idmx get <magnet>` works too.
* **Segments**: files are split into blocks fetched in parallel across networks; HLS/DASH segments are
  yt-dlp's job.

## The engine

* **Real interface binding.** Each connection is pinned to a network by source address *and* by
  device (`IP_BOUND_IF` on macOS, `SO_BINDTODEVICE` on Linux) — a source address alone doesn't stop
  the OS routing out of the default interface. On Windows only the source address is bound.
* **Work-stealing blocks.** The file is cut into 1–8 MB blocks; free connections take the next one,
  so fast networks do more of the work automatically.
* **Adaptive connections.** Each network ramps its connection count up while throughput keeps
  growing, and backs off when the server answers 403/429/503.
* **Tail hedging.** When no block is left waiting, a free connection may duplicate a block that a
  slow one is still fetching — only if it would clearly finish first — so one laggard can't hold the
  whole download back.
* **Safe resume.** Progress survives pause, crash and restart. Requests carry `If-Range`, so two
  versions of a file are never spliced. If the file changed while paused, IDMX stops and tells you
  (one click to start over) — or, with *Don't resume a changed file* off, restarts by itself.
* **IPv4 and IPv6.** The server's addresses are resolved once; each network uses the IP version it
  and the server share (IPv4 first), and falls back to the other the moment connecting fails. A
  network with nothing in common with the server says so instead of failing mysteriously.
* **Per-block attribution.** Every block records how many bytes each network delivered, so a block
  that started on one network and finished on another is credited to both (and drawn as bands).
* **Self-healing.** Failed blocks retry with jittered backoff, stalled connections are dropped, a
  network that keeps failing is parked and re-probed, expired signed URLs are refreshed,
  sleep/wake and address changes rebuild sockets immediately.
* **Networks come and go.** A network plugged in mid-download joins it *switched off* (so you choose),
  a network that drops out is shown as offline and rejoins by itself, and with no network at all
  downloads wait in the queue — and start on their own — instead of failing.
* **Plain-language errors.** Every failure says what happened and what to do about it ("The server
  won't let IDMX download this (403). The link may have expired…"), not `error sending request`.
* **Honest network status.** Each network reports *on*, *off*, *offline*, *unreachable* (can't get
  through) or *refused* (the server turned it away), with the error. Any network can be switched
  on or off **for a running download**; switching the last one off pauses it.
* **Integrity.** Optional checksum (`sha256:…`, a published `.sha256`, or `Digest` header) — a
  mismatch deletes the file and fails the job; it is never reported complete.

## Features

1. **CLI** — `idmx get <url…>` with a live per-network view; `idmx daemon`, `add`, `ls`, `pause`,
   `resume`, `nets`, `speedtest`, `probe`, `hash`. Patterns: `img[001-120].jpg`, `{a,b}.zip`.
2. **Queue & scheduler** — concurrency limit, priorities, start-at times, an overnight window.
3. **Browser extension** — right-click → *Download with IDMX*, or take over browser downloads,
   forwarding your cookies and referer.
4. **Per-network rules** — enable/disable, speed limits, metered links with data caps (daily or
   monthly), minimum file size, connection counts, names and colours.
5. **Checksums** — verify against a value you give, one the server publishes, or a Metalink.
6. **Batch** — pasted lists, URL patterns, Metalink (`.meta4`) files.
7. **Auth & proxy** — custom headers, cookies (or `cookies.txt`), basic/bearer auth, HTTP/SOCKS proxy.
8. **Network health** — live score, latency, quarantine and recovery; a **speed test** that measures
   each network, then all together, and explains a disappointing result (e.g. shared gateway).
9. **Post-download rules** — move, extract (zip/tar/tgz, zip-slip safe), run a command, reveal, notify;
   sleep or quit when the queue finishes.
10. **Remote dashboard** — the same UI over your LAN with a QR code; token-protected.
11. **Live per-download network switches**, and a **per-connection table** (every stream: network,
    state, chunk, speed — with racing connections marked).
12. **Ping & reachability** — handshake latency measured out of each network on its own, so a network
    with no internet shows "No internet" before a download ever starts.
13. **Same-subnet warning** — networks on one subnet share a link and won't add speed; IDMX says so.
14. **Update check** — asks GitHub's latest-release endpoint (opt-in: set `owner/repo` in Settings),
    shows a dismissible banner, never downloads or installs anything.
15. **Open network settings** from the app or `idmx net-settings`; **keep-awake** while downloading on
    macOS, Linux and Windows.
16. **Windows adapter names** via `Get-NetAdapter`, so an RNDIS phone isn't mistaken for Ethernet.

## Honest limits

* Combining networks only helps if they have **separate internet connections**. Two interfaces on the
  same router share one uplink. IDMX detects shared gateways and says so.
* The server must support HTTP range requests; otherwise IDMX falls back to a single connection.
* The IPv6 fallback is tested on loopback only; no IPv6-only network was available to test on.
* A checksum mismatch can't tell *which* block is bad (that needs per-piece hashes), so the whole file
  is re-fetched.
* Video: plain-file videos use the multi-network engine (split into blocks like any file). HLS/DASH
  streams are downloaded segment-by-segment by **yt-dlp**, on one connection, not split across networks.
* Torrents use librqbit; peer traffic can be bound to one chosen network, but one torrent isn't split
  across several the way an HTTP file is.
* The browser extension can't be installed from the Chrome Web Store (no listing). IDMX unpacks it and
  opens `chrome://extensions`; you switch on Developer mode, then drag the highlighted extension folder onto the page (or **Load unpacked**) once. After that it
  pairs itself through Chrome's native-messaging host (macOS/Linux; on Windows paste the token).
* The extension's logic is unit-tested under Node, but hasn't been driven in a real Chrome here.
* `--simulate` adds *fake* extra networks on the same connection, for demos/UI work only. In that
  mode you can type `offline`, `online`, `plug` or `unplug` into the daemon to play with the network list.
* Windows (keep-awake, adapter names, network settings) is type-checked but has never been run;
  Windows device binding and the packaged installers are untested on this machine.

## Security

The API requires a token on every call (even on loopback — a website can make your browser talk to
`localhost`) and only allows cross-origin requests from browser extensions and the app itself.
Settings that can run code or widen access (post-download commands, remote access, proxy, download
folder) can only be changed from the machine running IDMX, never over the network.

## Tests

```bash
cargo test --workspace      # engine, API and CLI, including a fault-injecting local origin
npm --prefix ui run typecheck
```

MIT licensed.
