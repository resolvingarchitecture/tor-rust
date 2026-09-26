//! # tor-client (Rust)
//!
//! A Tor client for **1M5**, in one of three modes ([`Mode`], config key
//! `ra.tor.mode`):
//!
//! | mode | behaviour |
//! |------|-----------|
//! | `local` | attach to a Tor daemon already running on this host (SOCKS `9050`, control `9051`) |
//! | `embedded` | run [Arti](https://gitlab.torproject.org/tpo/core/arti), the pure-Rust Tor client, in-process (requires the `embedded` feature) |
//! | `auto` (default) | `local` if a daemon is running, else `embedded`; falls back to `embedded` at runtime if the daemon later disappears, and back to `local` when it returns |
//!
//! Requests go out as HTTP/1.1 GET (`http://` only — HTTPS needs a TLS crate,
//! see `TODO.md`): over a SOCKS5 tunnel in `local` mode, over an Arti stream in
//! `embedded` mode.
//!
//! Used as the Tor **protocol service** for `1m5-core-rust` (via
//! `onemfive_core::protocol::TorProtocolService`).
//!
//! ```no_run
//! use tor::TorClient;
//! use std::collections::HashMap;
//!
//! let client = TorClient::from_config(&HashMap::new());
//! if client.start() {
//!     let mut env = seda_bus_envelope();  // env.headers["url"] = "http://<onion>/"
//!     client.send(&mut env);
//! }
//! # fn seda_bus_envelope() -> tor::Envelope { unimplemented!() }
//! ```

mod detector;
#[cfg(feature = "embedded")]
mod embedded;
mod http;
mod socks;

pub use detector::{LocalTorDetector, DEFAULT_CONTROL_PORT, DEFAULT_HOST, DEFAULT_SOCKS_PORT};

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, AtomicU8, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[cfg(feature = "embedded")]
use std::sync::Mutex;

use log::{info, warn};

/// Re-exported so callers need one crate. `1m5-core-rust` provides its own.
pub use seda_bus::Envelope;

/// Backend selection. Maps to `tor-client-java`'s `ra.tor.mode` and mirrors
/// `i2p-client`'s `Mode`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Local,
    Embedded,
    Auto,
}

impl Mode {
    fn parse(s: &str) -> Mode {
        match s.trim().to_ascii_lowercase().as_str() {
            "local" => Mode::Local,
            "embedded" => Mode::Embedded,
            _ => Mode::Auto,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Connecting,
    Connected,
    Disconnected,
    Error,
}

fn status_from_u8(v: u8) -> Status {
    match v {
        1 => Status::Connecting,
        2 => Status::Connected,
        3 => Status::Error,
        _ => Status::Disconnected,
    }
}
fn status_to_u8(s: Status) -> u8 {
    match s {
        Status::Disconnected => 0,
        Status::Connecting => 1,
        Status::Connected => 2,
        Status::Error => 3,
    }
}

/// Which backend is currently serving requests.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Backend {
    None,
    Local,
    Embedded,
}

fn backend_from_u8(v: u8) -> Backend {
    match v {
        1 => Backend::Local,
        2 => Backend::Embedded,
        _ => Backend::None,
    }
}
fn backend_to_u8(b: Backend) -> u8 {
    match b {
        Backend::None => 0,
        Backend::Local => 1,
        Backend::Embedded => 2,
    }
}

/// How often `auto` mode re-checks for a returned local daemon while running on
/// the embedded backend.
const LOCAL_REPROBE_INTERVAL: Duration = Duration::from_secs(30);

/// A Tor client. See the crate docs for the mode model.
pub struct TorClient {
    detector: LocalTorDetector,
    request_timeout: Duration,
    mode: Mode,
    data_dir: Option<PathBuf>,
    status: AtomicU8,
    active: AtomicU8,
    /// Epoch millis of the last local-daemon probe (for `auto` re-probe rate
    /// limiting). 0 = never.
    last_local_probe: AtomicU64,
    #[cfg(feature = "embedded")]
    embedded: Mutex<Option<embedded::EmbeddedTor>>,
}

impl TorClient {
    pub fn new() -> TorClient {
        TorClient {
            detector: LocalTorDetector::default(),
            request_timeout: Duration::from_secs(60),
            mode: Mode::Auto,
            data_dir: None,
            status: AtomicU8::new(status_to_u8(Status::Disconnected)),
            active: AtomicU8::new(backend_to_u8(Backend::None)),
            last_local_probe: AtomicU64::new(0),
            #[cfg(feature = "embedded")]
            embedded: Mutex::new(None),
        }
    }

    /// Config keys: `ra.tor.mode` (`local`/`embedded`/`auto`), `ra.tor.host`,
    /// `ra.tor.socksPort`, `ra.tor.controlPort`, `ra.tor.requestTimeoutSecs`,
    /// `ra.tor.dataDir` (embedded Arti state).
    pub fn from_config(cfg: &HashMap<String, String>) -> TorClient {
        let mut c = TorClient::new();
        if let Some(m) = cfg.get("ra.tor.mode") {
            c.mode = Mode::parse(m);
        }
        if let Some(h) = cfg.get("ra.tor.host") {
            c.detector.host = h.clone();
        }
        if let Some(p) = cfg.get("ra.tor.socksPort").and_then(|s| s.parse().ok()) {
            c.detector.socks_port = p;
        }
        if let Some(p) = cfg.get("ra.tor.controlPort").and_then(|s| s.parse().ok()) {
            c.detector.control_port = p;
        }
        if let Some(t) = cfg
            .get("ra.tor.requestTimeoutSecs")
            .and_then(|s| s.parse().ok())
        {
            c.request_timeout = Duration::from_secs(t);
        }
        if let Some(d) = cfg.get("ra.tor.dataDir") {
            c.data_dir = Some(PathBuf::from(d));
        }
        c
    }

    pub fn detector(&self) -> &LocalTorDetector {
        &self.detector
    }

    pub fn mode(&self) -> Mode {
        self.mode
    }

    pub fn status(&self) -> Status {
        status_from_u8(self.status.load(Ordering::Acquire))
    }

    fn set_status(&self, s: Status) {
        self.status.store(status_to_u8(s), Ordering::Release);
    }

    fn active(&self) -> Backend {
        backend_from_u8(self.active.load(Ordering::Acquire))
    }

    fn set_active(&self, b: Backend) {
        self.active.store(backend_to_u8(b), Ordering::Release);
    }

    /// Resolve [`Mode::Auto`] to a concrete backend for [`start`](Self::start).
    fn effective_mode(&self) -> Mode {
        match self.mode {
            Mode::Auto => {
                self.mark_local_probed();
                if self.detector.is_local_tor_running() {
                    Mode::Local
                } else {
                    Mode::Embedded
                }
            }
            m => m,
        }
    }

    /// Start the selected backend. Returns `false` cleanly (never panics or
    /// blocks indefinitely) if Tor is unavailable.
    pub fn start(&self) -> bool {
        self.set_status(Status::Connecting);

        match self.effective_mode() {
            Mode::Local => {
                if !self.detector.is_local_tor_running() {
                    warn!(
                        "No local Tor daemon on {} (SOCKS {} reachable={}, control {} reachable={}). \
                         Install and run Tor with 'ControlPort 9051', or use ra.tor.mode=auto/embedded.",
                        self.detector.host,
                        self.detector.socks_port,
                        self.detector.is_socks_reachable(),
                        self.detector.control_port,
                        self.detector.is_control_reachable(),
                    );
                    self.set_status(Status::Disconnected);
                    return false;
                }
                info!(
                    "Local Tor daemon reachable (SOCKS {}, control {}).",
                    self.detector.socks_port, self.detector.control_port
                );
                self.set_active(Backend::Local);
                self.set_status(Status::Connected);
                true
            }
            Mode::Embedded => match self.activate_embedded() {
                Ok(()) => {
                    self.set_status(Status::Connected);
                    true
                }
                Err(e) => {
                    warn!("embedded Tor (Arti) unavailable: {e}");
                    // A bootstrap failure with the feature built in is an error;
                    // the feature simply not being compiled in is not.
                    self.set_status(if cfg!(feature = "embedded") {
                        Status::Error
                    } else {
                        Status::Disconnected
                    });
                    false
                }
            },
            Mode::Auto => unreachable!("resolved by effective_mode"),
        }
    }

    pub fn stop(&self) -> bool {
        #[cfg(feature = "embedded")]
        {
            *self.embedded.lock().unwrap() = None; // Drop stops the Arti runtime
        }
        self.set_active(Backend::None);
        self.set_status(Status::Disconnected);
        true
    }

    /// Fetch `envelope.headers["url"]` through Tor into `envelope.payload`.
    /// HTTP only for now. On error, records `envelope.headers["error"]`.
    pub fn send(&self, envelope: &mut Envelope) -> bool {
        let Some(url) = envelope.headers.get("url").cloned() else {
            envelope
                .headers
                .insert("error".into(), "no url header".into());
            return false;
        };

        // In auto mode, if we're on the embedded backend, cheaply check whether
        // the local daemon has come back and, if so, switch to it.
        if self.mode == Mode::Auto && self.active() == Backend::Embedded {
            self.maybe_switch_back_to_local();
        }

        match self.active() {
            Backend::Local => match self.fetch_local(&url) {
                Ok(body) => {
                    envelope.payload = body;
                    true
                }
                Err(e) => {
                    // Only fall back if the daemon itself has gone away — not on
                    // a bad onion / unreachable host, where local is still fine.
                    if self.mode == Mode::Auto
                        && !self.probe_local()
                        && self.activate_embedded().is_ok()
                    {
                        warn!("local Tor daemon unreachable ({e}); switched to embedded Arti");
                        return self.fetch_embedded_into(&url, envelope);
                    }
                    warn!("Tor request to {url} failed: {e}");
                    envelope.headers.insert("error".into(), e.to_string());
                    false
                }
            },
            Backend::Embedded => self.fetch_embedded_into(&url, envelope),
            Backend::None => {
                envelope
                    .headers
                    .insert("error".into(), "Tor client not started".into());
                false
            }
        }
    }

    fn fetch_local(&self, url: &str) -> std::io::Result<Vec<u8>> {
        http::fetch_via_socks(
            &self.detector.host,
            self.detector.socks_port,
            url,
            self.request_timeout,
        )
    }

    fn fetch_embedded_into(&self, url: &str, envelope: &mut Envelope) -> bool {
        match self.fetch_embedded(url) {
            Ok(body) => {
                envelope.payload = body;
                true
            }
            Err(e) => {
                warn!("Tor (embedded) request to {url} failed: {e}");
                envelope.headers.insert("error".into(), e.to_string());
                false
            }
        }
    }

    // -- local-daemon probing (rate-limited for auto re-probe) ---------------

    fn mark_local_probed(&self) {
        self.last_local_probe.store(now_millis(), Ordering::Release);
    }

    /// Probe the local daemon now and record the time.
    fn probe_local(&self) -> bool {
        self.mark_local_probed();
        self.detector.is_local_tor_running()
    }

    /// If it's been at least [`LOCAL_REPROBE_INTERVAL`] since the last probe and
    /// the local daemon is back, switch the active backend to it (keeping the
    /// embedded client warm so a flapping daemon doesn't cost repeated
    /// bootstraps).
    fn maybe_switch_back_to_local(&self) {
        let last = self.last_local_probe.load(Ordering::Acquire);
        if now_millis().saturating_sub(last) < LOCAL_REPROBE_INTERVAL.as_millis() as u64 {
            return;
        }
        if self.probe_local() {
            info!("local Tor daemon is back; switching off embedded Arti");
            self.set_active(Backend::Local);
        }
    }

    // -- embedded backend --------------------------------------------------

    #[cfg(feature = "embedded")]
    fn activate_embedded(&self) -> std::io::Result<()> {
        let mut guard = self.embedded.lock().unwrap();
        if guard.is_none() {
            let base = self
                .data_dir
                .clone()
                .unwrap_or_else(|| PathBuf::from("."))
                .join("tor");
            *guard = Some(embedded::EmbeddedTor::start(base)?);
        }
        drop(guard);
        self.set_active(Backend::Embedded);
        Ok(())
    }

    #[cfg(feature = "embedded")]
    fn fetch_embedded(&self, url: &str) -> std::io::Result<Vec<u8>> {
        let guard = self.embedded.lock().unwrap();
        let tor = guard
            .as_ref()
            .ok_or_else(|| std::io::Error::other("embedded Tor not started"))?;
        tor.fetch(url, self.request_timeout)
    }

    #[cfg(not(feature = "embedded"))]
    fn activate_embedded(&self) -> std::io::Result<()> {
        Err(std::io::Error::other(
            "embedded Tor requires the `embedded` feature (Arti); build tor-client-rust \
             with --features embedded, or run a local Tor daemon",
        ))
    }

    #[cfg(not(feature = "embedded"))]
    fn fetch_embedded(&self, _url: &str) -> std::io::Result<Vec<u8>> {
        Err(std::io::Error::other("embedded Tor feature not enabled"))
    }
}

impl Default for TorClient {
    fn default() -> Self {
        Self::new()
    }
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode_parses() {
        assert_eq!(Mode::parse("local"), Mode::Local);
        assert_eq!(Mode::parse("EMBEDDED"), Mode::Embedded);
        assert_eq!(Mode::parse("whatever"), Mode::Auto);
    }

    #[test]
    fn status_round_trips() {
        for s in [
            Status::Connecting,
            Status::Connected,
            Status::Disconnected,
            Status::Error,
        ] {
            assert_eq!(status_from_u8(status_to_u8(s)), s);
        }
    }

    #[test]
    fn backend_round_trips() {
        for b in [Backend::None, Backend::Local, Backend::Embedded] {
            assert_eq!(backend_from_u8(backend_to_u8(b)), b);
        }
    }
}
