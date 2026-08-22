//! Where an output's audio goes (ADR 0028).
//!
//! The rules that matter here are about CLOCKS, not about sound: a sound device
//! has its own crystal, so changing which device an output uses is a hardware
//! change that restarts its render group — exactly as changing a DeckLink port
//! is (ADR 0024) — and an output's audio has exactly one destination, because
//! the per-frame sample count is corrected against exactly one buffer.

use pmg_node::runtime::{config_from_spec, plan_outputs, spec_from_config, OutputSpec};
use pmg_proto::OutputConfig;
use std::collections::BTreeSet;

fn out(id: &str, device: &str) -> OutputConfig {
    OutputConfig {
        id: id.into(),
        backend: "ndi".into(),
        ndi_name: format!("PMG {id}"),
        group: format!("g{id}"),
        audio_channels: 2,
        audio_device: device.into(),
        width: 1920,
        height: 1080,
        fps_num: 60000,
        fps_den: 1001,
        ..Default::default()
    }
}

fn current(cfgs: &[OutputConfig]) -> Vec<OutputSpec> {
    cfgs.iter().map(|c| spec_from_config(c, None)).collect()
}

#[test]
fn changing_the_sound_device_restarts_that_group_and_only_that_group() {
    let cfgs = [out("a", "BlackHole 2ch"), out("b", "Dante Virtual Soundcard")];
    let mut wanted = cfgs.clone();
    wanted[0].audio_device = "Dante Virtual Soundcard".into();

    let plan = plan_outputs(&current(&cfgs), &wanted, &BTreeSet::new(), false);
    // Closing one device and opening another IS a hardware change.
    assert_eq!(plan.changed_groups, BTreeSet::from(["ga".to_string()]));
}

#[test]
fn moving_audio_back_into_the_video_is_also_a_hardware_change() {
    let cfgs = [out("a", "BlackHole 2ch")];
    let mut wanted = cfgs.clone();
    wanted[0].audio_device = String::new();

    let plan = plan_outputs(&current(&cfgs), &wanted, &BTreeSet::new(), false);
    assert_eq!(plan.changed_groups, BTreeSet::from(["ga".to_string()]));
    assert_eq!(plan.next[0].audio_device, "");
}

#[test]
fn the_host_is_part_of_the_identity_a_name_alone_is_not_a_device() {
    // "Speakers" on WASAPI and "Speakers" on ASIO are different devices with
    // different clocks. Treating the pair as one would leave a reconfiguration
    // silently pointing at the wrong one.
    let cfgs = [out("a", "Speakers")];
    let mut wanted = cfgs.clone();
    wanted[0].audio_host = "ASIO".into();

    let plan = plan_outputs(&current(&cfgs), &wanted, &BTreeSet::new(), false);
    assert_eq!(plan.changed_groups, BTreeSet::from(["ga".to_string()]));
}

#[test]
fn an_unchanged_destination_interrupts_nothing() {
    let cfgs = [out("a", "BlackHole 2ch"), out("b", "")];
    let plan = plan_outputs(&current(&cfgs), &cfgs, &BTreeSet::new(), false);
    assert!(plan.changed_groups.is_empty(), "{:?}", plan.changed_groups);
}

#[test]
fn the_destination_survives_the_round_trip_through_the_wire_type() {
    // The node's own file is the authority for its hardware (ADR 0024), so a
    // destination that did not survive being written and read would silently
    // revert to embedded on the first restart.
    let spec = spec_from_config(&out("a", "Dante Virtual Soundcard"), None);
    let back = spec_from_config(&config_from_spec(&spec), Some(&spec));
    assert_eq!(back.audio_device, "Dante Virtual Soundcard");
    assert_eq!(back.audio_channels, 2);
}
