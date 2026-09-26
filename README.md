# tor-client (Rust)

A Tor client for **1M5**. Runs in one of three modes ([`Mode`], config key
`ra.tor.mode`):

| mode | behaviour |
|------|-----------|
| `local` | attach to a Tor daemon already running on this host — SOCKS5 `127.0.0.1:9050`, control port `127.0.0.1:9051` probed for readiness |
| `embedded` | run [Arti](https://gitlab.torproject.org/tpo/core/arti), the Tor Project's pure-Rust Tor client, in-process (requires the `embedded` feature) |
| `auto` *(default)* | `local` if a daemon is running, else `embedded`; **falls back to `embedded` at runtime** if the daemon later disappears, and back to `local` when it returns |

A Rust port of [`tor-client-java`](https://github.com/resolvingarchitecture/tor-client-java),
used as the Tor **protocol service** for
[`1m5-core-rust`](https://github.com/1m5/1m5-core-rust) via
`onemfive_core::protocol::TorProtocolService`.

The `local` / `embedded` / `auto` split mirrors
[`i2p-rust`](https://github.com/resolvingarchitecture/i2p-rust)'s `ra.i2p.mode`.

**Note a real divergence from `tor-client-java` as of 2026-09-26:** that port
no longer has a `local` mode at all - it never attaches to a pre-existing Tor
instance, only the one it downloads, verifies, and spawns itself (see its
README.md "Trust model"). This port's `auto` default still *prefers* `local`
when a daemon is reachable and will switch back to it at runtime if one
reappears while running on `embedded` (see "Which mode?" below for the
original reasoning). That was a deliberate, reasoned design choice before this
divergence existed; it hasn't been changed here, and shouldn't be without a
decision from whoever owns this port - see `TODO.md` P0.5.

## Which mode?

- A host that runs and maintains its own Tor (Tor Browser, a system `tor`
  service, Whonix/Tails) — `auto` attaches to it; that instance is patched by
  its own updates and its circuits are the ones you already trust.
- A host with no Tor, or where 1M5 should own the Tor path regardless (an
  appliance, an OS-level router, Redox) — `auto` (or `embedded`) runs Arti
  in-process. Arti's directory + guard state is persisted under
  `ra.tor.dataDir`, so only the first start pays the ~10–30s bootstrap.
- `embedded` pulls a large dependency tree (tokio, rustls, the `tor-*` crates)
  and is **off by default**; build with `--features embedded`.

## Local Tor daemon setup (`local` / `auto`)

Install Tor (`apt install tor`, `brew install tor`, …) and make sure
`/etc/tor/torrc` (or `~/.torrc`) has:

```
SocksPort 9050
ControlPort 9051
CookieAuthentication 0
```

Then `systemctl start tor` (or `tor -f ~/.torrc`). Check:
`curl --socks5-hostname 127.0.0.1:9050 https://check.torproject.org/api/ip`.

## Use

```rust
use std::collections::HashMap;
use tor_client::{Status, TorClient};

let client = TorClient::from_config(&HashMap::new());   // auto mode
if client.start() {                       // false (cleanly) if Tor is unavailable
    assert_eq!(client.status(), Status::Connected);
    let mut env = /* seda_bus::Envelope */;
    env.headers.insert("url".into(), "http://example.onion/".into());
    client.send(&mut env);                // body -> env.payload, errors -> env.headers["error"]
}
```

### Config keys

| key | default | meaning |
|-----|---------|---------|
| `ra.tor.mode` | `auto` | `local` / `embedded` / `auto` |
| `ra.tor.host` | `127.0.0.1` | local daemon host |
| `ra.tor.socksPort` | `9050` | local daemon SOCKS5 proxy port |
| `ra.tor.controlPort` | `9051` | local daemon control port (probed only) |
| `ra.tor.dataDir` | `.` | base dir for embedded Arti state (`<dir>/tor`) |
| `ra.tor.requestTimeoutSecs` | `60` | per-request timeout |

## Build

```
cargo test                                   # default: local backend only, light
cargo clippy --all-targets
cargo build   --features embedded            # + embedded Arti (heavy)
cargo test    --features embedded -- --ignored embedded_bootstraps_and_fetches
```

## Status

Early. HTTP (`http://`) over both backends works; HTTPS needs a TLS crate at the
request layer (see `TODO.md`). The local daemon's control port is only probed,
not spoken — no event stream or hidden-service management yet. Inbound / onion
hosting is not implemented. See `DESIGN.md` and `TODO.md`.
