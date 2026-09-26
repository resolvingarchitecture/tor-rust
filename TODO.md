# tor-client (Rust) — TODO

## P0 — embedded backend (done)
- [x] `Mode { Local, Embedded, Auto }` / `ra.tor.mode` (mirrors `i2p-rust`).
- [x] Embedded Arti backend behind the `embedded` feature (`arti-client`,
      rustls + bundled sqlite); persisted state under `ra.tor.dataDir`.
- [x] Runtime fallback in `auto`: `local`↔`embedded`, inline in `send()`.

## P0.5 — Reconcile with tor-client-java's zero-external-reliance model (decision needed)

`tor-client-java` no longer has a `local` mode at all as of 2026-09-26 - it
never attaches to a pre-existing Tor instance, period (see its README.md
"Trust model" / DESIGN.md "Why embedded"). This port's `auto` default does the
opposite of that on purpose: it *prefers* an already-running system daemon
over Arti, and switches back to it at runtime if one reappears. That was a
deliberate, reasoned choice (see README.md "Which mode?" - trusting a
host-managed Tor's own updates and circuits), not an oversight, and this repo
is genuinely ahead of the others in having real in-process embedding at all
via Arti. Not touched by this note - flagging the conflict for a real
decision, not applying one:

- [ ] Decide whether this port should follow suit (default to `embedded`,
      or drop `local`/`auto` entirely) for parity with `tor-client-java`'s new
      posture, or keep the current `auto` behavior as an intentional,
      documented difference for Rust specifically (e.g. because Arti makes
      "embed always" cheaper to default to here than a spawned-binary
      approach would be elsewhere, so the trust trade-off this repo already
      reasoned through may still be the better default even under the new
      framing). Either answer needs to be a deliberate choice, written down
      here and in DESIGN.md, not silently inherited from before this
      divergence existed.
- [ ] Readiness: report `Connecting`→`Connected` from Arti's bootstrap events
      instead of blocking `create_bootstrapped` (parallels `i2p-rust` P2).
- [ ] Drop the warm embedded client after a grace period once back on `local`
      (currently kept until `stop()`), if memory matters more than re-bootstrap.
- [ ] Bridges / pluggable transports passthrough for the embedded backend
      (obfs4, Snowflake) — needed for censored networks; `arti` PT maturity TBD.

## P1 — request path
- [ ] HTTPS for both backends (`rustls` at the request layer behind a `tls`
      feature; the embedded backend already links rustls).
- [ ] Follow redirects; surface status code + headers on the `Envelope`.
- [ ] Reuse the SOCKS connection / a small pool instead of one per request.
- [ ] Configurable `User-Agent`; strip identifying headers by default.

## P2 — Tor control protocol
- [ ] Port `TORControlConnection` / `TORControlCommands` from `tor-client-java`
      (authenticate with `CookieAuthentication 0` or a control password).
- [ ] Async event stream (`SETEVENTS`) → map `CIRC` / `STATUS_CLIENT` onto
      `Status`; live readiness instead of a one-shot probe.
- [ ] `NEWNYM` (new circuit) on demand and per ManCon escalation.

## P3 — inbound / hidden service
- [ ] Create or load an onion service key, `ADD_ONION` via the control port.
- [ ] Accept connections on the HS target port, turn requests into `Envelope`s
      and hand them to the bus (mirrors `tor-client-java`'s HS handler).

## P4 — privacy hardening
- [ ] Stream isolation: distinct SOCKS credentials per identity / destination.
- [ ] Assert the daemon's `SocksPort` has no `PreferSOCKSNoAuth` surprises.
- [ ] Optional bridge / pluggable-transport config passthrough.

## Testing / ops
- [ ] Integration test behind a `live` feature that uses a real local Tor.
- [x] `#[ignore]` live test: embedded Arti bootstraps + fetches over Tor.
- [ ] CI matrix: default, `--features embedded`; `cargo clippy -- -D warnings`,
      `cargo fmt --check`.
- [ ] Publish to crates.io once the API settles (currently git dep only).

## Cross-repo
- [ ] Keep `Status` and config keys aligned with `tor-client-java` 1.2.x and the
      `onemfive_core::protocol::TorProtocolService` adapter.
- [ ] `1m5-core-rust`: add a `tor-embedded` feature forwarding
      `tor_client/embedded` (mirrors the existing `i2p-embedded`).
- [ ] `1m505`: confirm `arti-client` builds for `x86_64-unknown-redox`
      (tokio + rustls/ring); assess Arti bridge support.
