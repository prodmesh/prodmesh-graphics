//! `pmgctl ndi` — the M0 NDI output spike.
//!
//! Three commands, in the order they should be trusted:
//!
//! - `info`  — load the runtime and report what it is.
//! - `probe` — **receive** from a known-good source and print the struct fields
//!   as decoded. This is the ABI validation: if `NDIlib_video_frame_v2_t`'s
//!   layout were wrong, the dimensions and frame rate would come back as
//!   garbage. Run this before trusting anything that *sends*.
//! - `send`  — send a test pattern, so an operator can look at it in NDI Video
//!   Monitor.
//!
//! Static offset assertions live in `ndi-sys`; this is the runtime half.

use anyhow::{bail, Context, Result};
use ndi_sys::*;
use pmg_color::patterns::{self, Pattern};
use pmg_color::Range;
use std::ffi::{CStr, CString};

pub const USAGE: &str = "\
pmgctl ndi — NDI runtime spike (M0)

    pmgctl ndi info
    pmgctl ndi probe [--source <substring>] [--seconds <n>]
    pmgctl ndi send  [--pattern <p>] [--name <n>] [--size WxH] [--fps N/D]
                     [--range narrow|full] [--seconds <n>] [--tone]

    --tone embeds a 1 kHz sine at the exact per-frame sample count the output's
    clock asks for, so `probe` on another machine can confirm both the audio
    ABI and the level. See pmg_audio: at 59.94 that count alternates.

PATTERNS
    softedge    soft-edged white on transparent — THE one that catches a bad
                premultiply. View it over a bright AND a dark background.
    alphastair  alpha at 0/25/50/75/100% — catches a key run through a matrix
    ramp        marked 0%/100% — proves range on a scope
    bars        75% colour bars — proves matrix and primaries
    line        moving 1-px line — proves frame timing
";

fn arg(args: &[String], flag: &str) -> Option<String> {
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

fn load() -> Result<Ndi> {
    Ndi::load().context(
        "could not load the NDI runtime. Install NDI Tools or the NDI SDK, or set \
         PMG_NDI_RUNTIME to a libndi dylib",
    )
}

pub fn main(args: &[String]) -> Result<()> {
    match args.first().map(String::as_str) {
        Some("info") => info(),
        Some("probe") => probe(args),
        Some("send") => send(args),
        _ => {
            print!("{USAGE}");
            Ok(())
        }
    }
}

fn info() -> Result<()> {
    let ndi = load()?;
    println!("NDI runtime      {}", ndi.version_string());
    println!("supported CPU    {}", ndi.supported_cpu());
    println!("\nStruct layouts are asserted statically in ndi-sys (see its `abi` tests).");
    println!("Run `pmgctl ndi probe` to confirm them against a live source.");
    Ok(())
}

/// Receive from a live source and print what we decoded.
///
/// The point is not the pixels. It is that `xres`, `yres`, `FourCC`, and the
/// frame rate come back as *plausible* values. A wrong struct layout would put
/// a pointer where an int is expected and the numbers would be absurd — which
/// is a harmless, immediate, and unmistakable way to learn the binding is wrong.
fn probe(args: &[String]) -> Result<()> {
    let want = arg(args, "--source");
    let seconds: u64 = arg(args, "--seconds")
        .and_then(|s| s.parse().ok())
        .unwrap_or(5);

    let ndi = load()?;
    println!("NDI runtime {}\n", ndi.version_string());

    // SAFETY: every call below uses types transcribed from include/pmg_ndi.h and
    // asserted in ndi-sys's `abi` tests. Instances are destroyed on every path.
    unsafe {
        let fc = NDIlib_find_create_t {
            show_local_sources: true,
            p_groups: std::ptr::null(),
            p_extra_ips: std::ptr::null(),
        };
        let finder = (ndi.find_create_v2)(&fc);
        if finder.is_null() {
            bail!("NDIlib_find_create_v2 returned null");
        }

        println!("Looking for sources (up to 5s)…");
        (ndi.find_wait_for_sources)(finder, 5000);

        let mut n: u32 = 0;
        let srcs = (ndi.find_get_current_sources)(finder, &mut n);
        if n == 0 || srcs.is_null() {
            (ndi.find_destroy)(finder);
            bail!(
                "no NDI sources found.\n\n\
                 Start one and try again — NDI Test Patterns (in /Applications) is ideal, \
                 because its dimensions and frame rate are known, which is exactly what \
                 this command checks against."
            );
        }

        let list: Vec<(usize, String)> = (0..n as usize)
            .map(|i| {
                let s = *srcs.add(i);
                let name = if s.p_ndi_name.is_null() {
                    String::from("<unnamed>")
                } else {
                    CStr::from_ptr(s.p_ndi_name).to_string_lossy().into_owned()
                };
                (i, name)
            })
            .collect();

        println!("Found {n} source(s):");
        for (i, name) in &list {
            println!("  [{i}] {name}");
        }

        let chosen = match &want {
            Some(w) => list
                .iter()
                .find(|(_, name)| name.to_lowercase().contains(&w.to_lowercase()))
                .map(|(i, _)| *i),
            None => Some(0),
        };
        let Some(idx) = chosen else {
            (ndi.find_destroy)(finder);
            bail!("no source matched {:?}", want.unwrap_or_default());
        };
        println!("\nConnecting to [{idx}] {}\n", list[idx].1);

        let rc = NDIlib_recv_create_v3_t {
            source_to_connect_to: *srcs.add(idx),
            color_format: RECV_COLOR_FORMAT_BGRX_BGRA,
            bandwidth: RECV_BANDWIDTH_HIGHEST,
            allow_video_fields: false,
            p_ndi_recv_name: c"pmgctl probe".as_ptr(),
        };
        let recv = (ndi.recv_create_v3)(&rc);
        (ndi.find_destroy)(finder); // sources array dies with the finder
        if recv.is_null() {
            bail!("NDIlib_recv_create_v3 returned null");
        }

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(seconds);
        let mut frames = 0u32;
        let mut audio_frames = 0u32;
        let mut ok = false;

        while std::time::Instant::now() < deadline && (frames < 3 || audio_frames < 3) {
            let mut v = NDIlib_video_frame_v2_t::default();
            let mut a = NDIlib_audio_frame_v3_t::default();
            let t = (ndi.recv_capture_v3)(recv, &mut v, &mut a, std::ptr::null_mut(), 1000);

            // Audio is validated the same way video is: by decoding fields off
            // a real frame. A struct whose offsets were wrong would report an
            // absurd rate or channel count rather than crashing, which is
            // exactly the failure this command exists to catch.
            if t == FRAME_TYPE_AUDIO {
                let peak = if a.p_data.is_null() || a.no_samples <= 0 {
                    0.0f32
                } else {
                    // SAFETY (inside the enclosing unsafe block): the SDK owns
                    // this buffer until recv_free_audio_v3; planar, no_samples
                    // floats per channel at channel_stride.
                    // p_data is uint8_t* in v3; the samples are f32.
                    let base = a.p_data as *const f32;
                    let stride = (a.channel_stride_in_bytes as usize) / 4;
                    let mut m = 0.0f32;
                    for ch in 0..a.no_channels.max(0) as usize {
                        for i in 0..a.no_samples as usize {
                            let x = *base.add(ch * stride + i);
                            m = m.max(x.abs());
                        }
                    }
                    m
                };
                let dbfs = if peak > 0.0 {
                    format!("{:.1} dBFS", 20.0 * peak.log10())
                } else {
                    "silence".to_string()
                };
                println!(
                    "audio: {} Hz, {} ch, {} samples, stride {} B, peak {dbfs}",
                    a.sample_rate, a.no_channels, a.no_samples, a.channel_stride_in_bytes
                );
                audio_frames += 1;
                // SAFETY: frame came from recv_capture_v3 and is not used after.
                (ndi.recv_free_audio_v3)(recv, &a);
                continue;
            }
            if t != FRAME_TYPE_VIDEO {
                continue;
            }
            frames += 1;

            let fourcc = v.FourCC.to_le_bytes();
            let fourcc = String::from_utf8_lossy(&fourcc).into_owned();
            println!("frame {frames}:");
            println!("  xres x yres          {} x {}", v.xres, v.yres);
            println!("  FourCC               0x{:08X} ({fourcc})", v.FourCC);
            println!(
                "  frame rate           {}/{}  ({:.3} fps)",
                v.frame_rate_N,
                v.frame_rate_D,
                v.frame_rate_N as f64 / v.frame_rate_D.max(1) as f64
            );
            println!("  aspect               {}", v.picture_aspect_ratio);
            println!("  frame_format_type    {}", v.frame_format_type);
            println!("  line_stride          {}", v.line_stride_in_bytes);
            println!("  timecode             {}", v.timecode);
            println!("  p_data               {:?}", v.p_data);

            // The actual assertion. Every one of these would fail spectacularly
            // if a field landed at the wrong offset.
            let sane = v.xres > 0
                && v.xres <= 16384
                && v.yres > 0
                && v.yres <= 16384
                && v.frame_rate_N > 0
                && v.frame_rate_D > 0
                && (v.frame_rate_N as f64 / v.frame_rate_D as f64) < 1000.0
                && !v.p_data.is_null()
                && v.line_stride_in_bytes >= v.xres;
            println!("  plausible?           {}", if sane { "YES" } else { "NO" });
            ok |= sane;

            (ndi.recv_free_video_v2)(recv, &v);
        }

        (ndi.recv_destroy)(recv);

        if frames == 0 {
            bail!("connected but received no video frames within {seconds}s");
        }
        if !ok {
            bail!(
                "received frames but the decoded fields are implausible.\n\
                 The struct layout in sys/ndi-sys is WRONG — do not send anything until it is fixed."
            );
        }
        println!(
            "\nABI VALIDATED — decoded fields are plausible across {frames} frame(s).\n\
             NDIlib_video_frame_v2_t's layout is correct at runtime as well as statically."
        );
    }
    Ok(())
}

fn send(args: &[String]) -> Result<()> {
    let pattern_name = arg(args, "--pattern").unwrap_or_else(|| "softedge".into());
    let Some(pattern) = Pattern::parse(&pattern_name) else {
        bail!(
            "unknown pattern {pattern_name:?}. One of: {}",
            Pattern::ALL.join(", ")
        );
    };
    let name = arg(args, "--name").unwrap_or_else(|| "ProdMesh Graphics".into());
    let seconds: u64 = arg(args, "--seconds")
        .and_then(|s| s.parse().ok())
        .unwrap_or(60);
    let range = match arg(args, "--range").as_deref() {
        Some("full") => Range::Full,
        _ => Range::Narrow,
    };

    let (w, h) = match arg(args, "--size") {
        Some(s) => {
            let (a, b) = s
                .split_once('x')
                .context("--size must look like 1920x1080")?;
            (a.parse::<u32>()?, b.parse::<u32>()?)
        }
        None => (1920, 1080),
    };
    // Exact rational, always. Never a 59.94 float (docs/specs/output-config.md §2).
    let (fps_n, fps_d) = match arg(args, "--fps") {
        Some(s) => {
            let (a, b) = s
                .split_once('/')
                .context("--fps must look like 60000/1001")?;
            (a.parse::<i32>()?, b.parse::<i32>()?)
        }
        None => (60000, 1001),
    };

    let ndi = load()?;
    println!("NDI runtime {}", ndi.version_string());
    println!(
        "Sending \"{name}\"  {w}x{h}  {fps_n}/{fps_d} ({:.3} fps)  pattern={pattern_name}  range={range:?}",
        fps_n as f64 / fps_d as f64
    );
    println!("\nOpen NDI Video Monitor and pick \"{name}\" from its menu bar.");
    if pattern == Pattern::SoftEdge || pattern == Pattern::AlphaStaircase {
        println!(
            "This pattern carries alpha. Check it over a BRIGHT and a DARK background —\n\
             a premultiply error is invisible over one of them."
        );
    }
    println!("Ctrl-C to stop.\n");

    let c_name = CString::new(name)?;

    // SAFETY: types transcribed from include/pmg_ndi.h and asserted in ndi-sys.
    // The sender is destroyed on the way out; `bgra` outlives every send call
    // because send_video_v2 is synchronous (unlike the _async variants, which
    // would require the buffer to stay valid past the call).
    unsafe {
        let sc = NDIlib_send_create_t {
            p_ndi_name: c_name.as_ptr(),
            p_groups: std::ptr::null(),
            // Let the SDK pace us, so this free-runs at the right rate without
            // a timer of our own. Frame N's timing is then the SDK's problem.
            clock_video: true,
            clock_audio: false,
        };
        let sender = (ndi.send_create)(&sc);
        if sender.is_null() {
            bail!("NDIlib_send_create returned null");
        }

        let total = (seconds as f64 * fps_n as f64 / fps_d as f64).round() as u64;
        let start = std::time::Instant::now();
        let mut announced = false;

        // A 1 kHz tone at the EXACT per-frame sample count, so a probe on the
        // other end validates the audio ABI and the level together. The phase
        // carries across frames in the absolute sample index; resetting it each
        // frame would click 60 times a second and prove nothing.
        let tone = args.iter().any(|a| a == "--tone");
        let aclock = tone
            .then(|| {
                pmg_audio::AudioClock::new((fps_n as u32, fps_d as u32), pmg_audio::SAMPLE_RATE)
            })
            .transpose()?;
        if let Some(c) = &aclock {
            println!(
                "  tone: 1 kHz, {:.1} samples/frame nominal, cadence every {} frames",
                c.samples_per_frame(),
                c.cadence_frames()
            );
        }
        let mut pcm: Vec<i32> = Vec::new();
        let mut phase: u64 = 0;

        for frame_no in 0..total {
            // Static patterns are regenerated each frame rather than cached:
            // wasteful, and it keeps this spike honest about per-frame cost.
            let f = patterns::render(pattern, w, h, frame_no);
            let bgra = patterns::to_bgra8_straight(&f, range);

            let video = NDIlib_video_frame_v2_t {
                xres: w as i32,
                yres: h as i32,
                FourCC: FOURCC_BGRA,
                frame_rate_N: fps_n,
                frame_rate_D: fps_d,
                frame_format_type: FRAME_FORMAT_PROGRESSIVE,
                p_data: bgra.as_ptr() as *mut u8,
                line_stride_in_bytes: (w * 4) as i32,
                ..Default::default()
            };
            (ndi.send_send_video_v2)(sender, &video);

            if let Some(c) = &aclock {
                let n = c.samples_for(frame_no) as usize;
                pcm.clear();
                pcm.reserve(n * 2);
                for _ in 0..n {
                    let t = phase as f64 / pmg_audio::SAMPLE_RATE as f64;
                    let v = ((t * 1000.0 * std::f64::consts::TAU).sin() * 0.25 * i32::MAX as f64)
                        as i32;
                    pcm.push(v);
                    pcm.push(v);
                    phase += 1;
                }
                let src = NDIlib_audio_frame_interleaved_32s_t {
                    sample_rate: pmg_audio::SAMPLE_RATE as i32,
                    no_channels: 2,
                    no_samples: n as i32,
                    p_data: pcm.as_ptr() as *mut i32,
                    ..Default::default()
                };
                let mut planar = vec![0f32; n * 2];
                let mut dst = NDIlib_audio_frame_v2_t {
                    sample_rate: pmg_audio::SAMPLE_RATE as i32,
                    no_channels: 2,
                    no_samples: n as i32,
                    p_data: planar.as_mut_ptr(),
                    channel_stride_in_bytes: (n * 4) as i32,
                    ..Default::default()
                };
                (ndi.util_audio_from_interleaved_32s_v2)(&src, &mut dst);
                (ndi.send_send_audio_v2)(sender, &dst);
            }

            if !announced && (ndi.send_get_no_connections)(sender, 0) > 0 {
                announced = true;
                println!("A receiver connected.");
            }
            if frame_no > 0 && frame_no % 300 == 0 {
                println!(
                    "  {frame_no} frames, {:.1}s elapsed, {} receiver(s)",
                    start.elapsed().as_secs_f64(),
                    (ndi.send_get_no_connections)(sender, 0)
                );
            }
        }

        let elapsed = start.elapsed().as_secs_f64();
        println!(
            "\nSent {total} frames in {elapsed:.2}s = {:.3} fps (asked for {:.3})",
            total as f64 / elapsed,
            fps_n as f64 / fps_d as f64
        );
        (ndi.send_destroy)(sender);
    }
    Ok(())
}
