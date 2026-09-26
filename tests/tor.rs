use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::thread;
use std::time::Duration;

use tor::{LocalTorDetector, Status, TorClient};

#[test]
fn detector_reports_nothing_when_ports_closed() {
    let d = LocalTorDetector {
        socks_port: 9098,
        control_port: 9099,
        timeout: Duration::from_millis(300),
        ..Default::default()
    };
    assert!(!d.is_socks_reachable());
    assert!(!d.is_control_reachable());
    assert!(!d.is_local_tor_running());
}

#[cfg(not(feature = "embedded"))]
#[test]
fn start_fails_cleanly_without_a_daemon_or_embedded() {
    // auto mode (default), no daemon, `embedded` feature not built -> clean
    // failure, not an error. (With the feature built, auto would bootstrap Arti
    // here - see the ignored live test below.)
    let mut cfg = HashMap::new();
    cfg.insert("ra.tor.socksPort".into(), "9098".into());
    cfg.insert("ra.tor.controlPort".into(), "9099".into());
    let client = TorClient::from_config(&cfg);
    assert!(!client.start());
    assert_eq!(client.status(), Status::Disconnected);
}

/// Live: `embedded` mode actually bootstraps Arti and fetches over Tor.
/// Needs network + ~10-30s. Run with:
/// `cargo test --features embedded -- --ignored embedded_bootstraps`
#[cfg(feature = "embedded")]
#[test]
#[ignore]
fn embedded_bootstraps_and_fetches() {
    let dir = std::env::temp_dir().join(format!("ra-tor-it-{}", std::process::id()));
    let mut cfg = HashMap::new();
    cfg.insert("ra.tor.mode".into(), "embedded".into());
    cfg.insert("ra.tor.dataDir".into(), dir.display().to_string());
    let client = TorClient::from_config(&cfg);
    assert!(client.start(), "embedded Arti should bootstrap");
    assert_eq!(client.status(), Status::Connected);

    let mut env = seda_bus::Envelope::new("x", Vec::new());
    env.headers
        .insert("url".into(), "http://example.com/".into());
    assert!(client.send(&mut env), "fetch over Tor should succeed");
    assert!(!env.payload.is_empty());
    client.stop();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn local_mode_without_a_daemon_is_disconnected() {
    let mut cfg = HashMap::new();
    cfg.insert("ra.tor.mode".into(), "local".into());
    cfg.insert("ra.tor.socksPort".into(), "9098".into());
    cfg.insert("ra.tor.controlPort".into(), "9099".into());
    let client = TorClient::from_config(&cfg);
    assert_eq!(client.mode(), tor::Mode::Local);
    assert!(!client.start());
    assert_eq!(client.status(), Status::Disconnected);
}

#[test]
fn socks_connect_and_http_fetch_through_a_fake_proxy() {
    // A fake SOCKS5 proxy that also serves the "destination" HTTP response.
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let socks_port = listener.local_addr().unwrap().port();
    // separate listener just so the control-port probe passes
    let control = TcpListener::bind("127.0.0.1:0").unwrap();
    let control_port = control.local_addr().unwrap().port();
    thread::spawn(move || {
        for _ in control.incoming() { /* accept + drop */ }
    });

    thread::spawn(move || {
        // The detector probes the SOCKS port before the real request, so accept
        // in a loop and handle whichever connection completes a handshake.
        for conn in listener.incoming() {
            let Ok(mut s) = conn else { continue };
            // greeting (probe connections close here -> handshake fails, keep going)
            let mut g = [0u8; 3];
            if s.read_exact(&mut g).is_err() {
                continue;
            }
            s.write_all(&[0x05, 0x00]).unwrap();
            // connect request: 4 bytes head + 1 len + name + 2 port
            let mut head = [0u8; 5];
            s.read_exact(&mut head).unwrap();
            let mut name = vec![0u8; head[4] as usize + 2];
            s.read_exact(&mut name).unwrap();
            // reply: success, ATYP=1, 0.0.0.0:0
            s.write_all(&[0x05, 0x00, 0x00, 0x01, 0, 0, 0, 0, 0, 0])
                .unwrap();
            // now act as the HTTP server
            let mut req = [0u8; 1024];
            let _ = s.read(&mut req).unwrap();
            s.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\nhello")
                .unwrap();
            return;
        }
    });

    let mut cfg = HashMap::new();
    cfg.insert("ra.tor.socksPort".into(), socks_port.to_string());
    cfg.insert("ra.tor.controlPort".into(), control_port.to_string());
    let client = TorClient::from_config(&cfg);
    assert!(client.start());

    let mut env = seda_bus::Envelope::new("x", Vec::new());
    env.headers
        .insert("url".into(), "http://example.onion/path".into());
    assert!(client.send(&mut env));
    assert_eq!(env.payload, b"hello");
}
