//! `pmgctl audio` — host audio devices, and the soak that proves the clock.
//!
//! The companion to `pmgctl decklink`. `list` answers "can this machine see my
//! Dante Virtual Soundcard"; `soak` answers the only question that matters
//! afterwards, which is whether audio stays with picture for two hours.

use anyhow::{bail, Result};
use pmg_out_audio as audio;

pub fn main(args: &[String]) -> Result<()> {
    match args.first().map(String::as_str) {
        Some("list") => list(),
        Some("soak") => soak(&args[1..]),
        _ => {
            usage();
            Ok(())
        }
    }
}

fn usage() {
    println!("pmgctl audio — host audio output devices");
    println!();
    println!("    pmgctl audio list              Hosts and their output devices");
    println!("    pmgctl audio soak [flags]      Feed a device against a video clock");
    println!("        --device NAME              Which device (default: the system default)");
    println!("        --host NAME                Which host API (default: the platform's)");
    println!("        --channels N               How many channels to send (default 2)");
    println!("        --minutes N                How long (default 1)");
    println!("        --fps NUM/DEN              The video clock to pace against");
    println!("                                   (default 60000/1001)");
    println!("        --free-run                 Do NOT correct drift. Shows what an");
    println!("                                   open loop actually costs.");
    println!();
    println!("The interesting number is `buf`, the sample frames the device still holds.");
    println!("FLAT means the video clock and the device's crystal are being held together.");
    println!("A steady climb means the device is slower than the video clock and the audio");
    println!("is walking late; falling ends in a click. Neither is reported as an error by");
    println!("anything else in the system, which is why this command exists.");
}

fn flag<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .map(String::as_str)
}

fn list() -> Result<()> {
    let hosts = audio::hosts();
    println!("host APIs        {}", hosts.len());
    for h in &hosts {
        println!("  {}{}", h.name, if h.default { "  (default)" } else { "" });
    }
    println!();

    let devices = audio::devices();
    if devices.is_empty() {
        println!("No audio output devices.");
        println!();
        println!("On a machine that should have some, this is usually a virtual driver that");
        println!("failed to load rather than absent hardware. Check the OS sound settings");
        println!("first: if the system cannot see it either, this is not the place to fix it.");
        return Ok(());
    }
    println!("output devices   {}", devices.len());
    for d in &devices {
        println!(
            "  [{}] {}{}",
            d.host,
            d.name,
            if d.default { "  (default)" } else { "" }
        );
        println!(
            "        {} channels   48 kHz {}",
            d.channels,
            if d.supports_48k {
                "yes"
            } else {
                "NO — this device cannot carry our audio"
            }
        );
    }
    println!();
    println!("Dante Virtual Soundcard appears here like any other device once it is running.");
    println!("If it is installed and absent, start it — DVS only registers its device while");
    println!("the application is up.");
    Ok(())
}

/// Feed a device from a video clock, and report what the buffer does.
///
/// This is the audio half of spike S3. The bar is the same: over the soak,
/// **zero underruns**, and a buffer level that ends where it started.
fn soak(args: &[String]) -> Result<()> {
    let channels: u16 = flag(args, "--channels")
        .and_then(|v| v.parse().ok())
        .unwrap_or(2);
    let minutes: u64 = flag(args, "--minutes")
        .and_then(|v| v.parse().ok())
        .unwrap_or(1);
    let free_run = args.iter().any(|a| a == "--free-run");
    let (fps_num, fps_den) = match flag(args, "--fps") {
        None => (60000u32, 1001u32),
        Some(s) => {
            let (n, d) = s.split_once('/').unwrap_or((s, "1"));
            (n.parse()?, d.parse()?)
        }
    };

    let cfg = audio::Config {
        host: flag(args, "--host").map(str::to_string),
        device: flag(args, "--device").map(str::to_string),
        channels,
        // Three video frames, the depth the node uses.
        target_frames: (48_000 * fps_den * 3 / fps_num).max(256),
    };
    let sink = audio::Sink::open(&cfg)?;
    println!(
        "device           {} ({})",
        sink.device_name(),
        sink.host_name()
    );
    println!("clock            {fps_num}/{fps_den} fps, 48 kHz, {channels} ch");
    println!(
        "correction       {}",
        if free_run {
            "OFF — open loop, on purpose"
        } else {
            "AudioSync"
        }
    );
    println!();

    let aclock = pmg_audio::AudioClock::new((fps_num, fps_den), 48_000)
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    let mut sync: Option<pmg_audio::AudioSync> = None;

    let frames = (minutes * 60 * fps_num as u64) / fps_den as u64;
    let frame_dur =
        std::time::Duration::from_nanos((1_000_000_000u64 * fps_den as u64) / fps_num as u64);
    let start = std::time::Instant::now();
    // A 1 kHz tone, because a drifting buffer is audible in it and a silent
    // soak proves only that nothing crashed.
    let mut phase = 0f64;
    let mut buf: Vec<i32> = Vec::new();
    let mut first_buffered = 0u32;

    for frame in 0..frames {
        let nominal = aclock.samples_for(frame);
        let buffered = sink.buffered();
        // The baseline is read the same way in both modes, so the two runs are
        // comparable: an open-loop run that never recorded a starting depth
        // would report its whole fill as drift.
        if first_buffered == 0 && buffered > 0 && sink.primed() && frame > 240 {
            first_buffered = buffered;
            println!("baseline         {buffered} frames buffered");
            if !free_run {
                sync = Some(pmg_audio::AudioSync::new(buffered));
            }
        }
        let n = match (&mut sync, buffered) {
            (Some(s), b) if b > 0 => s.correct(nominal, b),
            _ => nominal,
        };

        buf.clear();
        for _ in 0..n {
            phase += 1000.0 * std::f64::consts::TAU / 48_000.0;
            let s = ((phase.sin() * 0.25) * i32::MAX as f64) as i32;
            for _ in 0..channels {
                buf.push(s);
            }
        }
        sink.push(&buf);

        // Pace to the video clock. A real node is paced by the video output
        // blocking; here the sleep stands in for it.
        let due = start + frame_dur * (frame as u32 + 1);
        if let Some(d) = due.checked_duration_since(std::time::Instant::now()) {
            std::thread::sleep(d);
        }

        if frame > 0 && frame.is_multiple_of(fps_num as u64 * 10 / fps_den as u64) {
            let s = sync.as_ref();
            println!(
                "{:>5}s  buf={:>5}  underruns={}  dropped={}  corrections={}  resyncs={}",
                start.elapsed().as_secs(),
                buffered,
                sink.underruns(),
                sink.dropped(),
                s.map(|s| s.corrections).unwrap_or(0),
                s.map(|s| s.resyncs).unwrap_or(0),
            );
        }
    }

    let ended = sink.buffered();
    println!();
    println!("buffer           {first_buffered} -> {ended} frames");
    let walked = ended as i64 - first_buffered as i64;
    println!(
        "drift            {walked} frames ({:.1} ms) over {minutes} minute(s)",
        walked as f64 * 1000.0 / 48_000.0,
    );
    println!("underruns        {}", sink.underruns());
    println!("dropped          {}", sink.dropped());
    if sink.underruns() > 0 {
        bail!(
            "FAIL — an underrun is an audible click, and there were {}",
            sink.underruns()
        );
    }
    println!();
    println!("PASS");
    Ok(())
}
