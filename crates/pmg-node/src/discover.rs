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

/// Browse for a control server, returning `host:port`.
///
/// Takes the first responder. A room with two control servers is a
/// misconfiguration this cannot resolve, and guessing between them would be
/// worse than the operator being told to name one — so the caller logs every
/// server it saw.
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
    let mut seen: Vec<String> = Vec::new();
    let mut first: Option<String> = None;

    while Instant::now() < deadline {
        let left = deadline.saturating_duration_since(Instant::now());
        match receiver.recv_timeout(left) {
            Ok(mdns_sd::ServiceEvent::ServiceResolved(info)) => {
                // Prefer an address we can actually dial. A hostname from mDNS
                // needs the responder to still be answering when we connect;
                // the address it already gave us does not.
                let addr = info
                    .get_addresses()
                    .iter()
                    .next()
                    .map(|a| a.to_string())
                    .unwrap_or_else(|| info.get_hostname().trim_end_matches('.').to_string());
                let hostport = format!("{addr}:{}", info.get_port());
                if !seen.contains(&hostport) {
                    seen.push(hostport.clone());
                }
                if first.is_none() {
                    first = Some(hostport);
                    // Keep listening briefly so a second server is REPORTED
                    // rather than silently ignored — an operator with two
                    // servers on one network needs to know.
                    std::thread::sleep(Duration::from_millis(300).min(left));
                    break;
                }
            }
            Ok(_) => {}
            Err(_) => break,
        }
    }
    let _ = daemon.shutdown();
    first.map(|f| (f, seen))
}
