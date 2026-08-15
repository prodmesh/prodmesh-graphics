//! Finding a control server without being told where it is.
//!
//! # An aid, never an authority
//!
//! `--server` always wins and always works. Multicast is genuinely unreliable
//! across VLANs and behind switches with poor IGMP snooping, so an operator who
//! can type an address must never be blocked by a broken multicast domain
//! (`docs/specs/pmg-link.md` §1). Everything here is best-effort: it returns
//! `None` and the caller says so plainly rather than failing.

use std::time::{Duration, Instant};

/// The service the control server advertises.
pub const SERVICE: &str = "_pmg-control._tcp.local.";

/// Browse for a control server, returning `host:port` and everything seen.
///
/// Collects for the whole window rather than returning on the first responder,
/// so a room with two control servers is REPORTED. A node cannot arbitrate
/// between them — guessing would decide what goes on air — but the operator can,
/// and only if they are told.
pub fn find(timeout: Duration) -> Option<(String, Vec<String>)> {
    let daemon = match mdns_sd::ServiceDaemon::new() {
        Ok(d) => d,
        Err(e) => {
            tracing::warn!(error = %e, "mDNS is unavailable; use --server");
            return None;
        }
    };
    let receiver = match daemon.browse(SERVICE) {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(error = %e, "could not browse for a control server; use --server");
            return None;
        }
    };

    let deadline = Instant::now() + timeout;
    // Keep every candidate address per instance, so the preference below has
    // something to choose from rather than whichever arrived first.
    let mut found: Vec<(String, Vec<String>, u16)> = Vec::new();
    let mut first_seen_at: Option<Instant> = None;

    while Instant::now() < deadline {
        // Once something has answered, wait only a short grace period for a
        // second — long enough to notice a duplicate, short enough that the
        // common case of one server is not slowed to the full timeout.
        if let Some(t) = first_seen_at {
            if t.elapsed() > Duration::from_millis(600) {
                break;
            }
        }
        let left = deadline.saturating_duration_since(Instant::now());
        match receiver.recv_timeout(left.min(Duration::from_millis(200))) {
            Ok(mdns_sd::ServiceEvent::ServiceResolved(info)) => {
                let name = info.get_fullname().to_string();
                if found.iter().any(|(n, _, _)| n == &name) {
                    continue;
                }
                found.push((
                    name,
                    // `ScopedIp` is not Copy and prints with its zone when it
                    // has one, which is exactly the detail best_address weighs.
                    info.get_addresses().iter().map(|a| a.to_string()).collect(),
                    info.get_port(),
                ));
                first_seen_at.get_or_insert_with(Instant::now);
            }
            Ok(_) => {}
            Err(_) => {
                // A timeout with something already found means the field is
                // probably quiet; with nothing found we keep waiting out the
                // window.
                if first_seen_at.is_some() {
                    break;
                }
            }
        }
    }
    let _ = daemon.shutdown();

    let all: Vec<String> = found
        .iter()
        .filter_map(|(_, addrs, port)| best_address(addrs).map(|a| format!("{a}:{port}")))
        .collect();
    all.first().cloned().map(|f| (f, all))
}

/// Pick the address most likely to still work tomorrow.
///
/// **IPv4 first, then routable IPv6, and link-local IPv6 only as a last
/// resort.** An `fe80::…` address needs a zone index (`%en0`) that is
/// meaningful only on the interface it came from, and this address does not
/// just dial the control socket — it becomes the base URL for CAS fetches and
/// the host in the preview URL handed to a browser. A link-local address with a
/// percent sign in it is a poor thing to put in a URL, and on a multi-homed
/// machine it is the wrong interface half the time.
fn best_address(addrs: &[String]) -> Option<String> {
    let ipv4 = |s: &String| s.parse::<std::net::Ipv4Addr>().is_ok();
    let link_local = |s: &String| s.to_ascii_lowercase().starts_with("fe80:");

    addrs
        .iter()
        .find(|a| ipv4(a))
        .or_else(|| addrs.iter().find(|a| !link_local(a)))
        .or_else(|| addrs.first())
        .map(|a| {
            // An IPv6 literal needs brackets in a URL, and the CAS base URL is
            // built by formatting this straight into one.
            if a.contains(':') {
                format!("[{a}]")
            } else {
                a.clone()
            }
        })
}
