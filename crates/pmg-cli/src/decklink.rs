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

use anyhow::{bail, Result};
use decklink_sys as dl;
use pmg_color::patterns::{self, Pattern};
use pmg_color::Range;
use std::collections::BTreeMap;

pub const USAGE: &str = "\
pmgctl decklink — DeckLink enumeration and capability probe (M0)

    pmgctl decklink list             Devices and their probed capabilities
    pmgctl decklink modes [index]    Display modes a device reports
    pmgctl decklink ref [index]      Reference (genlock) status
    pmgctl decklink out [flags]      Spike S3's soak: scheduled output
        --device N --minutes N --preroll N --pattern P --mode WxHpNUM/DEN
        --audio N                    Embed N channels (2, 8 or 16) of a 1 kHz
                                     tone, and report the card's audio buffer
        --audio-naive                Deliberately feed floor(samples/frame)
                                     instead of the exact cadence, to show what
                                     a drifting buffer looks like

With --audio the interesting number is `abuf`, the card's own buffered sample
count. Audio runs continuous, so a FLAT level means the samples-per-frame
cadence is exact. Climbing means too many samples per frame and lip sync
walking late; falling ends in a click. Nothing reports either as an error.

The soak's bar is ZERO late and ZERO dropped over ten minutes at 1080p59.94.
One late frame in ten minutes is one visible glitch per ten minutes of service.
See docs/spikes/S3-decklink-output.md.
";

pub fn main(args: &[String]) -> Result<()> {
    match args.first().map(String::as_str) {
        Some("list") => list(),
        Some("modes") => modes(index_arg(args)),
        Some("ref") => reference(index_arg(args)),
        Some("out") => out(args),
        _ => {
            print!("{USAGE}");
            Ok(())
        }
    }
}

fn arg(args: &[String], flag: &str) -> Option<String> {
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1))
        .cloned()
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
        println!("No DeckLink devices found.");
        println!();
        // Zero devices with a loaded framework is almost always a half-finished
        // driver update, not absent hardware — and it presents identically to a
        // cable falling out. Measured 2026-08-14; see docs/INTEGRATION-NOTES.md.
        println!("If you just updated Desktop Video, this is probably the driver extension");
        println!("rather than the hardware. Check:");
        println!();
        println!("    systemextensionsctl list | grep -i blackmagic");
        println!();
        println!("A line reading [activated waiting for user] means macOS is holding the new");
        println!("extension pending approval, while the OLD one is still the live driver — so");
        println!("a new framework is talking to an old dext and nothing enumerates.");
        println!();
        println!("Fix: System Settings > Privacy & Security, approve the Blackmagic Design");
        println!("system software, then REBOOT.");
        println!();
        println!("IF YOU ALREADY REBOOTED AND IT IS STILL ZERO: systemextensionsctl reports");
        println!("what is REGISTERED, not what is RUNNING. Check which one actually has the");
        println!("card:");
        println!();
        println!("    ps -ax -o args= | grep BlackmagicIO.DExt");
        println!("    ls -ldT /Library/SystemExtensions/<uuid-from-that-path>");
        println!();
        println!("If the running bundle is older than the one listed as [activated enabled],");
        println!("the old extension is deadlocked holding the PCIe device and a reboot will");
        println!("not break it. Unplug the Thunderbolt chassis for ~10s and plug it back in;");
        println!("removing the device is what lets the old extension release.");
        println!();
        println!("Otherwise check the Thunderbolt connection and that Desktop Video Setup");
        println!("sees the device.");
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

/// `pmgctl decklink out` — spike S3's soak.
///
/// **Frames are pre-rendered into a pool and scheduled from it.** Generating per
/// frame would put 17.55 ms of CPU work in the loop (measured over the NDI path,
/// see `docs/INTEGRATION-NOTES.md`) and we would be measuring the pattern
/// generator rather than DeckLink's scheduling — a useless soak.
fn out(args: &[String]) -> Result<()> {
    preamble();

    let index: i32 = arg(args, "--device")
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let minutes: f64 = arg(args, "--minutes")
        .and_then(|s| s.parse().ok())
        .unwrap_or(10.0);
    let preroll: i32 = arg(args, "--preroll")
        .and_then(|s| s.parse().ok())
        .unwrap_or(3);
    let pattern_name = arg(args, "--pattern").unwrap_or_else(|| "line".into());
    let Some(pattern) = Pattern::parse(&pattern_name) else {
        bail!(
            "unknown pattern {pattern_name:?}. One of: {}",
            Pattern::ALL.join(", ")
        );
    };

    let dev = match dl::Device::open(index) {
        Ok(d) => d,
        Err(e) => {
            println!("\n{e}");
            return Ok(());
        }
    };

    // 1080p59.94 unless told otherwise — the format the success bar is written
    // against, and the one with the tightest budget this card offers.
    let want = arg(args, "--mode").unwrap_or_else(|| "1920x1080p60000/1001".into());
    let modes = dev.modes();
    let Some(mode) = modes.iter().find(|m| fmt_mode(m) == want).cloned() else {
        println!("\nNo mode matching {want:?}. Available:");
        for m in &modes {
            println!("  {}", fmt_mode(m));
        }
        return Ok(());
    };

    let fps = mode.fps();
    let budget_ms = 1000.0 / fps;
    let total: u64 = (minutes * 60.0 * fps).round() as u64;

    println!(
        "\nsoak: {} for {minutes} min = {total} frames",
        fmt_mode(&mode)
    );
    println!("  per-frame budget   {budget_ms:.3} ms");
    println!("  preroll            {preroll} frames");
    println!("  pattern            {pattern_name} (pre-rendered pool, not generated per frame)");

    // Pre-render a short cycle of the pattern.
    //
    // CAP THIS. A full width-length sweep at 1080p is 1920 frames x 8.29 MB =
    // ~16 GB, which is reckless on a dev box and fatal on a booth machine. The
    // line's job here is to reveal tearing and stutter, and a one-second loop
    // does that as well as a full sweep.
    const MAX_POOL_FRAMES: u64 = 60;
    let pool_len = if pattern == Pattern::MovingLine {
        (mode.width as u64).min(MAX_POOL_FRAMES)
    } else {
        1
    };
    let pool_mb = (mode.width as u64 * mode.height as u64 * 4 * pool_len) / (1024 * 1024);
    println!("  pre-rendering      {pool_len} frame(s), ~{pool_mb} MB…");
    let t0 = std::time::Instant::now();
    let pool: Vec<Vec<u8>> = (0..pool_len)
        .map(|i| {
            let f = patterns::render(pattern, mode.width as u32, mode.height as u32, i);
            patterns::to_bgra8_straight(&f, Range::Narrow)
        })
        .collect();
    println!("  pre-rendered in    {:.1}s\n", t0.elapsed().as_secs_f64());

    let audio_channels: i32 = arg(args, "--audio")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    // The clock that decides how many samples ride with each frame. Built from
    // the OUTPUT's rate, which is the whole point — see pmg_audio.
    let aclock = (audio_channels > 0)
        .then(|| {
            pmg_audio::AudioClock::new(
                (mode.fps_num as u32, mode.fps_den as u32),
                pmg_audio::SAMPLE_RATE,
            )
        })
        .transpose()?;
    // Deliberately wrong, so the instrument can be shown to work. A flat buffer
    // level only means something if a drifting one is visibly different.
    let naive = args.iter().any(|a| a == "--audio-naive");
    if let Some(c) = &aclock {
        println!(
            "  audio              {audio_channels} ch, {:.1} samples/frame nominal, \
             cadence repeats every {} frames",
            c.samples_per_frame(),
            c.cadence_frames()
        );
        if naive {
            println!(
                "  AUDIO NAIVE        feeding a flat {} samples/frame ON PURPOSE — \
                 the buffer should DRAIN",
                c.samples_per_frame().floor() as u64
            );
        }
    }

    let output = match dl::Output::open_with_audio(
        &dev,
        &mode,
        dl::KeyMode::FillOnly,
        preroll,
        audio_channels,
    ) {
        Ok(o) => o,
        Err(e) => {
            println!("could not open output: {e}");
            println!();
            println!("If this is an interface error, the installed Desktop Video driver is");
            println!("older than the SDK. The enumeration path tolerates that with a fallback");
            println!("chain; the OUTPUT path deliberately does not, because its methods sit");
            println!("past the common vtable prefix. Update Desktop Video to match the SDK.");
            return Ok(());
        }
    };

    let start = std::time::Instant::now();
    let mut scheduled = 0u64;
    let mut backoffs = 0u64;

    // A 1 kHz tone, generated once per frame into a reused buffer. Generating
    // it per frame is fine — it is a few hundred samples of sine, not a
    // per-pixel pass — but the PHASE must be continuous across frames or the
    // tone clicks every frame and the test measures nothing.
    let mut tone: Vec<i32> = Vec::new();
    let mut tone_phase: u64 = 0;

    while scheduled < total {
        let buf = &pool[(scheduled % pool_len) as usize];

        if let Some(c) = &aclock {
            let frames = if naive {
                c.samples_per_frame().floor() as usize
            } else {
                c.samples_for(scheduled) as usize
            };
            tone.clear();
            tone.reserve(frames * audio_channels as usize);
            for _ in 0..frames {
                // 1 kHz at 48 kHz: 48 samples a cycle. Phase carried in the
                // absolute sample index so it never resets at a frame boundary.
                let t = tone_phase as f64 / pmg_audio::SAMPLE_RATE as f64;
                let v =
                    ((t * 1000.0 * std::f64::consts::TAU).sin() * 0.25 * i32::MAX as f64) as i32;
                for _ in 0..audio_channels {
                    tone.push(v);
                }
                tone_phase += 1;
            }
        }

        match output.schedule_bgra_with_audio(buf, &tone) {
            Ok(()) => scheduled += 1,
            // ONLY busy is backpressure. Every other error means something is
            // actually wrong, and treating it as "try again" turns a fault into
            // a hang: the first version of this looped for seven minutes on a
            // failing audio schedule, printing nothing, looking exactly like a
            // slow card. A loop that retries everything cannot tell you what
            // broke, and a soak that cannot tell you what broke is not an
            // instrument.
            Err(dl::Error::Failed(rc)) if rc != dl::PMG_DL_ERR_BUSY => {
                let detail = match output.last_failure() {
                    Some((call, hr)) => format!("{call} returned 0x{hr:08X}"),
                    None => format!("code {rc}"),
                };
                anyhow::bail!(
                    "scheduling failed after {scheduled} frames ({backoffs} back-offs): \
                     {detail}. This is not backpressure."
                );
            }
            Err(_) => {
                // Pool exhausted: DeckLink has not returned a frame yet. Back
                // off rather than spin — a busy-wait here would steal CPU from
                // the very thread we are measuring.
                //
                // Sleep a quarter-frame rather than a fixed 500 us. At 500 us
                // this loop woke ~17 times per frame to do nothing, which is
                // noise in the measurement and heat in the room.
                backoffs += 1;
                std::thread::sleep(std::time::Duration::from_micros((budget_ms * 250.0) as u64));
            }
        }

        if scheduled > 0 && scheduled.is_multiple_of(fps.round() as u64 * 30) {
            let s = output.stats();
            let audio = if audio_channels > 0 {
                format!(
                    " · abuf {:>5} ({:.0} ms)",
                    s.buffered_audio,
                    s.buffered_audio as f64 * 1000.0 / pmg_audio::SAMPLE_RATE as f64
                )
            } else {
                String::new()
            };
            println!(
                "  {:>6} sched · {:>6} done · late {} · dropped {} · buffered {}{audio} · {:.1}s",
                s.scheduled,
                s.completed,
                s.displayed_late,
                s.dropped,
                s.buffered,
                start.elapsed().as_secs_f64()
            );
        }
    }

    // Time the scheduling window itself. The drain wait below must not be
    // counted, or the reported rate is depressed by a fixed half second.
    let sched_elapsed = start.elapsed().as_secs_f64();

    // Let the tail of the queue drain before reading final counters, or the
    // last few frames show as scheduled-but-not-completed and look like a fault.
    std::thread::sleep(std::time::Duration::from_millis(500));
    let s = output.stats();
    let elapsed = sched_elapsed;

    println!("\n--- result ---");
    println!("  elapsed            {elapsed:.2}s");
    println!("  scheduled          {}", s.scheduled);
    println!("  completed          {}", s.completed);
    println!("  DISPLAYED LATE     {}", s.displayed_late);
    println!("  dropped            {}", s.dropped);
    println!("  pool back-offs     {backoffs}");
    println!(
        "  effective rate     {:.3} fps (asked {fps:.3})",
        s.completed as f64 / elapsed
    );
    println!();
    if s.displayed_late == 0 && s.dropped == 0 {
        println!("PASS — zero late, zero dropped. Spike S3's internal-clock leg is met.");
        println!("Still outstanding, and both need the reference generator at church:");
        println!("  - the same soak locked to blackburst or tri-level");
        println!("  - scope confirmation that a fill+key pair really shares a clock");
    } else {
        println!("FAIL — the bar is zero. One late frame in ten minutes is one visible");
        println!("glitch per ten minutes of service. Check readback and preroll depth");
        println!("before concluding anything about the SDK.");
    }
    Ok(())
}

fn fmt_mode(m: &dl::Mode) -> String {
    format!(
        "{}x{}{}{}/{}",
        m.width,
        m.height,
        if m.interlaced { "i" } else { "p" },
        m.fps_num,
        m.fps_den
    )
}
