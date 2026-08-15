//! `pmgctl decklink` — enumeration and capability probing.
//!
//! The half of spike S3 that can be answered without writing the output path.
//! Its purpose is to produce facts for `docs/INTEGRATION-NOTES.md`: which
//! device, which modes, whether external keying is *actually* available, and
//! which connectors share a card and therefore a clock.
//!
//! **Everything here is probed, never assumed.** A model number does not
//! predict key+fill support (risk R4), which is why Configure will show what
//! the device reported rather than what the box says.

use anyhow::Result;
use decklink_sys as dl;
use std::collections::BTreeMap;

pub const USAGE: &str = "\
pmgctl decklink — DeckLink enumeration and capability probe (M0)

    pmgctl decklink list             Devices and their probed capabilities
    pmgctl decklink modes [index]    Display modes a device reports
    pmgctl decklink ref [index]      Reference (genlock) status

Output is NOT yet implemented — that is spike S3, and it gets built with the
ten-minute zero-late-frame soak rather than in passing. See
docs/spikes/S3-decklink-output.md.
";

pub fn main(args: &[String]) -> Result<()> {
    match args.first().map(String::as_str) {
        Some("list") => list(),
        Some("modes") => modes(index_arg(args)),
        Some("ref") => reference(index_arg(args)),
        _ => {
            print!("{USAGE}");
            Ok(())
        }
    }
}

fn index_arg(args: &[String]) -> i32 {
    args.get(1).and_then(|s| s.parse().ok()).unwrap_or(0)
}

fn preamble() {
    match dl::sdk_version() {
        Some(v) => println!("DeckLink SDK     {v} (shim compiled against it)"),
        None => println!("DeckLink SDK     not built in — set DECKLINK_SDK_DIR and rebuild"),
    }
    println!("driver loaded    {}", dl::driver_loaded());
    if let Some(r) = dl::interface_report() {
        println!("driver interfaces {r}");
        if r.contains("none") {
            println!();
            println!("!! At least one interface reported \"none\". The installed Desktop Video");
            println!("!! driver is older than the SDK this was built against, so QueryInterface");
            println!("!! failed and every capability below silently reads as its DEFAULT.");
            println!("!! Do not trust a \"no\" until the driver is updated.");
        }
    }
}

fn list() -> Result<()> {
    preamble();

    let n = match dl::device_count() {
        Ok(n) => n,
        Err(e) => {
            // The error says what to do about it, per docs/UI_TEXT.md.
            println!("\n{e}");
            return Ok(());
        }
    };
    println!("devices          {n}\n");
    if n == 0 {
        println!("No DeckLink devices found. Check the Thunderbolt connection and that");
        println!("Desktop Video Setup sees the device.");
        return Ok(());
    }

    // Group by TOPOLOGICAL id, not persistent id. Measured on a Quad 2:
    // persistent_id increments per connector, so grouping on it would invent
    // eight single-connector "cards" — and would let Configure offer a fill+key
    // "pair" spanning two cards, which looks fine and is out of phase.
    let mut cards: BTreeMap<i64, Vec<dl::DeviceInfo>> = BTreeMap::new();

    for i in 0..n {
        let info = dl::device_info(i)?;
        println!("[{i}] {}", info.display_name);
        println!("      model               {}", info.model_name);
        println!(
            "      sub-device          {} of {}",
            info.sub_device_index, info.sub_device_count
        );
        println!(
            "      external keying     {}  <- fill+key needs this",
            yn(info.supports_key_fill)
        );
        println!(
            "      internal keying     {}",
            yn(info.supports_internal_key)
        );
        println!("      reference input     {}", yn(info.has_reference_input));
        println!("      max audio channels  {}", info.max_audio_channels);
        println!(
            "      persistent id       0x{:X}  (per connector)",
            info.persistent_id
        );
        println!(
            "      topological id      0x{:X}  (the card)",
            info.topological_id
        );
        println!();
        // MEASURED HEURISTIC, not documented behaviour — see the note printed
        // below and docs/INTEGRATION-NOTES.md.
        cards
            .entry(info.topological_id >> 8)
            .or_default()
            .push(info);
    }

    println!("Grouped by topological id >> 8 — see the caveat below:");
    for (tid, conns) in &cards {
        let names: Vec<String> = conns.iter().map(|c| format!("[{}]", c.index)).collect();
        let keyable: Vec<i32> = conns
            .iter()
            .filter(|c| c.supports_key_fill)
            .map(|c| c.index)
            .collect();
        println!(
            "  group 0x{tid:X}: {} connector(s) {}",
            conns.len(),
            names.join(" ")
        );
        if keyable.is_empty() {
            println!("      no external keying on any connector — fill-only or embedded alpha");
        } else {
            let k: Vec<String> = keyable.iter().map(|i| format!("[{i}]")).collect();
            println!(
                "      external keying on {} — fill+key pairs live here",
                k.join(" ")
            );
            println!("      both halves of a pair schedule at the same display time on one");
            println!("      card, which is what actually guarantees phase (ADR 0011)");
        }
    }

    println!();
    println!("CAVEAT — this grouping is a measured heuristic, not documented behaviour.");
    println!("Both persistent_id and topological_id encode the sub-device in their low");
    println!("bits, so neither is directly \"the card\". Measured on this Quad 2:");
    println!("  persistent  0xC55080A0..A7   (one per connector)");
    println!("  topological 0x5005_00..03, 0x5007_00..03");
    println!("Before Configure offers a fill+key pair as phase-locked, spike S3 must");
    println!("CONFIRM on a scope that the two connectors really share a clock. Offering");
    println!("a pair that spans two cards would look fine and be out of phase.");
    Ok(())
}

fn modes(index: i32) -> Result<()> {
    preamble();
    let dev = match dl::Device::open(index) {
        Ok(d) => d,
        Err(e) => {
            println!("\n{e}");
            return Ok(());
        }
    };
    let modes = dev.modes();
    println!("device [{index}] reports {} display modes\n", modes.len());

    // Group by resolution so the list is readable rather than a wall of 200.
    let mut by_res: BTreeMap<(i32, i32), Vec<&dl::Mode>> = BTreeMap::new();
    for m in &modes {
        by_res.entry((m.width, m.height)).or_default().push(m);
    }
    for ((w, h), ms) in &by_res {
        let rates: Vec<String> = ms
            .iter()
            .map(|m| {
                format!(
                    "{}/{}{}",
                    m.fps_num,
                    m.fps_den,
                    if m.interlaced { "i" } else { "" }
                )
            })
            .collect();
        println!("  {w:>5} x {h:<5}  {}", rates.join("  "));
    }

    // The rates we care about most, and the reason fps is always rational.
    if let Some(m) = modes
        .iter()
        .find(|m| m.width == 1920 && m.height == 1080 && !m.interlaced && m.fps_num == 60000)
    {
        println!(
            "\n1080p59.94 is available as {}/{} = {:.4} fps exactly.",
            m.fps_num,
            m.fps_den,
            m.fps()
        );
        println!("Note the rational form — 60000/1001, never a 59.94 float. Timeline time is");
        println!("computed from the ratio, so an 800 ms In is 800 ms at any rate.");
    }
    Ok(())
}

fn reference(index: i32) -> Result<()> {
    preamble();
    let dev = match dl::Device::open(index) {
        Ok(d) => d,
        Err(e) => {
            println!("\n{e}");
            return Ok(());
        }
    };
    match dev.reference() {
        Some(r) => {
            println!("\ndevice [{index}] reference:");
            println!("  input present    {}", yn(r.present));
            println!("  locked           {}", yn(r.locked));
            println!(
                "  format           {}",
                if r.format.is_empty() { "-" } else { &r.format }
            );
            println!();
            if r.locked {
                println!("Locked. Outputs on THIS card share its clock, so a fill+key pair here");
                println!("is genuinely in phase.");
            } else {
                println!("Free-running. Reference gives PHASE, not frame numbering — so even when");
                println!("this locks, two nodes on the same generator still disagree about which");
                println!("frame is number 1841944. v1 does not promise cross-node simultaneous");
                println!("takes (ADR 0011).");
            }
        }
        None => println!("\nDevice reported no reference status."),
    }
    Ok(())
}

fn yn(b: bool) -> &'static str {
    if b {
        "yes"
    } else {
        "no"
    }
}
