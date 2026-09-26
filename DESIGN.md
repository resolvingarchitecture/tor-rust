# tor-client (Rust) — Design

A Tor client exposed as a small `TorClient` type that `1m5-core-rust` wraps as
the Tor **protocol service**. A Rust port of the design in
[`tor-client-java`](https://github.com/resolvingarchitecture/tor-client-java),
trimmed to what the router needs today, with an embedded backend added so it runs
where no system Tor exists.

## Where it sits

    1m5-core-rust  ──wraps──►  tor_client::TorClient
      onemfive_core::protocol::TorProtocolService (impl Service + Transport)
                                     │
                    ┌────────────────┴─────────────────┐
              local backend                      embedded backend
              SOCKS5 127.0.0.1:9050              arti_client::TorClient
              control 127.0.0.1:9051 (probe)     (pure-Rust Tor, in-process)
                    │                                   │
              system tor daemon                   the Tor network directly

`RoutingService` discovers the protocol service by name and pushes a
routing-slip hop carrying the destination URL; the adapter calls
`TorClient::send`, which dispatches to whichever backend is active.

## Divergence from tor-client-java (2026-09-26, unresolved)

`tor-client-java` retired its `local` (system-daemon) path entirely: it now
only ever runs a Tor process it downloaded, verified, and spawned itself, on
the reasoning that depending on some other already-running Tor instance
should never be a default, let alone the preferred one. This port's `auto`
default still does the opposite - see "Modes" below and README.md's "Which
mode?" for the (reasoned, not accidental) case for preferring a host-managed
Tor when one exists. That reasoning predates this divergence and hasn't been
revisited against it. See `TODO.md` P0.5 - this needs an explicit decision,
not a silent fix in either direction.

## Modes (`ra.tor.mode`)

`Mode { Local, Embedded, Auto }`, mirroring `i2p-rust`'s `Mode`.

- **`local`** — require a running daemon; `start()` fails fast (returns `false`,
  status `Disconnected`) if the SOCKS + control ports don't both answer.
- **`embedded`** — run Arti in-process (`embedded` feature; without it, `start()`
  returns `false` with status `Disconnected` and a build hint).
- **`auto`** (default) — `effective_mode()` probes the local daemon once at
  `start()`: reachable → `local`, else `embedded`. Then **runtime fallback**
  (below) keeps `auto` on the best available backend.

## Runtime fallback (auto only)

Inline in `send()` — no background threads, matching the "the bus stage owns the
thread" model.

- **local → embedded.** A `send()` over the local backend that fails is
  re-checked against the detector. If the daemon is *gone* (not merely a bad
  onion / unreachable host), `activate_embedded()` starts Arti, the active
  backend switches, and the request is retried once over embedded. Status stays
  `Connected` throughout — the router sees only a `NetworkStatus` no-op.
- **embedded → local.** While serving on embedded under `auto`, each `send()`
  calls `maybe_switch_back_to_local()`, which re-probes the local port at most
  once per `LOCAL_REPROBE_INTERVAL` (30s, rate-limited via an epoch-millis
  `AtomicU64`). When the daemon is back, the active backend flips to `local`.
  The embedded Arti client is kept warm (not dropped) so a flapping daemon
  doesn't cost repeated bootstraps; `stop()` drops it.

## Components

    LocalTorDetector   probes SOCKS + control ports (TcpStream::connect_timeout)
    socks              minimal SOCKS5 CONNECT client, no auth
    http               request/response helpers (parse_url, format_get,
                        split_body) shared by both backends; http:// only, no TLS
    embedded           (feature "embedded") EmbeddedTor: a Tokio runtime on
                        dedicated workers + a bootstrapped arti_client::TorClient;
                        Arti drives its own background tasks, so no driver thread
    TorClient          config, status + active-backend as AtomicU8, mode,
                        start()/stop()/send(), the fallback logic

## Message flow

**Outbound** — `1m5-core-rust` routes an `Envelope` whose `headers["url"]` is a
`.onion` or clearnet `http://` URL.

- *local*: `http::fetch_via_socks` opens a SOCKS5 tunnel through `127.0.0.1:9050`,
  issues `GET`, returns the body.
- *embedded*: `EmbeddedTor::fetch` does `client.connect((host, port)).await` for
  a `DataStream`, then the same `GET` over it, inside `runtime.block_on`.

Body → `envelope.payload`; any error string → `envelope.headers["error"]`.

**Inbound** — not implemented (see `TODO.md`); Arti's `onion-service-client`
feature is enabled for *connecting to* `.onion`, not hosting.

## Status model

`Status { Connecting, Connected, Disconnected, Error }` as an `AtomicU8`.

- `start()` → `Connecting`, then `Connected` once a backend is active, else
  `Disconnected` (nothing available) or `Error` (embedded feature built but Arti
  bootstrap failed).
- `stop()` → `Disconnected`, drops the embedded client.

The `1m5-core-rust` adapter maps this onto its own `NetworkStatus`; the
local/embedded switch is deliberately invisible there.

## Dependencies

Default build: `log` + `seda_bus` only. The `embedded` feature adds
`arti-client` (with `rustls` + bundled `static-sqlite`, avoiding system OpenSSL
and libsqlite3), `tor-rtcompat`, `tokio`, and `rustls` (ring provider, pinned so
Arti's TLS setup is unambiguous). Heavy — off by default.

## Rust adaptations vs. the Java client

- No inheritance / `NetworkService` base — `TorClient` is a plain struct; the
  bus lifecycle lives in `1m5-core-rust`'s adapter.
- No `TaskRunner` status poller — status is set at `start()`/`stop()`/`send()`.
- Hand-rolled SOCKS5 + HTTP for the local path instead of `reqwest` — keeps the
  default build dependency-light and blocking.
- The Tor control protocol client (`TORControlConnection` & friends) is still
  not ported — the control port is probed only.

## Not here

- HTTPS (needs `rustls`/`native-tls` at the request layer).
- Tor control protocol: authentication, event stream, `NEWNYM`, circuit info.
- Hidden service (onion) hosting for inbound envelopes.
- Stream isolation per identity / per destination.
- Bridges / pluggable transports for the embedded backend.
