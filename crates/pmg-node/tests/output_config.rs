//! Reconfiguring a node's outputs (ADR 0024).
//!
//! The rules, tested where they live — deciding what changes is separate from
//! doing it, so these run without spawning a render thread or opening a device.
//!
//! What matters: an output with a graphic on it is not interrupted by accident,
//! only the groups that actually change are torn down, and an output that has
//! not changed is left completely alone.

use pmg_node::runtime::{config_from_spec, plan_outputs, spec_from_config, OutputSpec};
use pmg_proto::OutputConfig;
use std::collections::BTreeSet;

fn ndi(id: &str, name: &str, group: &str, preview: u16) -> OutputConfig {
    OutputConfig {
        id: id.into(),
        backend: "ndi".into(),
        ndi_name: name.into(),
        preview_port: preview,
        group: group.into(),
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

fn nothing_on_air() -> BTreeSet<String> {
    BTreeSet::new()
}

#[test]
fn an_unchanged_output_is_not_touched() {
    let cfgs = [ndi("a", "Lab A", "ga", 8621), ndi("b", "Lab B", "gb", 8622)];
    let plan = plan_outputs(&current(&cfgs), &cfgs, &nothing_on_air(), false);
    assert!(
        plan.changed_groups.is_empty(),
        "re-sending the same configuration must interrupt nothing: {:?}",
        plan.changed_groups
    );
    assert_eq!(plan.next.len(), 2);
}

#[test]
fn only_the_group_that_changed_is_interrupted() {
    let cfgs = [
        ndi("a", "Lab A", "ga", 8621),
        ndi("b", "Lab B", "gb", 8622),
        ndi("c", "Lab C", "gc", 8623),
    ];
    let mut wanted = cfgs.clone();
    wanted[1].ndi_name = "Lab B renamed".into();

    let plan = plan_outputs(&current(&cfgs), &wanted, &nothing_on_air(), false);
    assert_eq!(
        plan.changed_groups,
        BTreeSet::from(["gb".to_string()]),
        "the wall is renamed; the key feed keeps its frames"
    );
}

/// A DeckLink-only field must not tear down an NDI sender. This cost a frame of
/// black on a live output the first time it was driven, because the display
/// mode string differed on an output that has never read one.
#[test]
fn a_field_the_backend_does_not_read_changes_nothing() {
    let cfgs = [ndi("a", "Lab A", "ga", 8621)];
    let mut wanted = cfgs.clone();
    wanted[0].mode = "1080p59.94".into();
    wanted[0].device = 7;
    wanted[0].key_device = Some(9);

    let plan = plan_outputs(&current(&cfgs), &wanted, &nothing_on_air(), false);
    assert!(
        plan.changed_groups.is_empty(),
        "an NDI output does not open a DeckLink device: {:?}",
        plan.changed_groups
    );
}

#[test]
fn an_output_with_a_graphic_on_it_is_refused_and_keeps_running() {
    let cfgs = [ndi("a", "Lab A", "ga", 8621), ndi("b", "Lab B", "gb", 8622)];
    let mut wanted = cfgs.clone();
    wanted[0].ndi_name = "Lab A renamed".into();

    let on_air = BTreeSet::from(["a".to_string()]);
    let plan = plan_outputs(&current(&cfgs), &wanted, &on_air, false);

    assert_eq!(plan.refused_on_air, vec!["a".to_string()]);
    assert!(
        plan.changed_groups.is_empty(),
        "and nothing was interrupted"
    );
    // The output stays exactly as it was — refusing has to mean keeping, or the
    // graphic goes off anyway and the refusal was theatre.
    let a = plan.next.iter().find(|o| o.id == "a").expect("still there");
    assert_eq!(a.ndi_name, "Lab A");
}

#[test]
fn force_reconfigures_the_output_that_is_on_air() {
    let cfgs = [ndi("a", "Lab A", "ga", 8621)];
    let mut wanted = cfgs.clone();
    wanted[0].ndi_name = "Lab A renamed".into();

    let plan = plan_outputs(
        &current(&cfgs),
        &wanted,
        &BTreeSet::from(["a".to_string()]),
        true,
    );
    assert!(plan.refused_on_air.is_empty());
    assert_eq!(plan.changed_groups, BTreeSet::from(["ga".to_string()]));
    assert_eq!(plan.next[0].ndi_name, "Lab A renamed");
}

#[test]
fn an_output_that_leaves_takes_its_group_down() {
    let cfgs = [ndi("a", "Lab A", "ga", 8621), ndi("b", "Lab B", "gb", 8622)];
    let plan = plan_outputs(&current(&cfgs), &cfgs[..1], &nothing_on_air(), false);
    assert_eq!(plan.changed_groups, BTreeSet::from(["gb".to_string()]));
    assert_eq!(plan.next.len(), 1);
}

/// Moving an output between groups has to rebuild BOTH — the one it left is
/// still driving the device otherwise.
#[test]
fn moving_an_output_between_groups_rebuilds_both() {
    let cfgs = [ndi("a", "Lab A", "ga", 8621), ndi("b", "Lab B", "gb", 8622)];
    let mut wanted = cfgs.clone();
    wanted[1].group = "ga".into();

    let plan = plan_outputs(&current(&cfgs), &wanted, &nothing_on_air(), false);
    assert_eq!(
        plan.changed_groups,
        BTreeSet::from(["ga".to_string(), "gb".to_string()])
    );
}

/// Layers belong to the assignment, not to this message. A reconfiguration
/// must not quietly reset a layer stack an operator built.
#[test]
fn layers_survive_a_hardware_change() {
    let cfgs = [ndi("a", "Lab A", "ga", 8621)];
    let mut running = current(&cfgs);
    running[0].layers = vec!["Lower Third".into(), "Bug".into()];

    let mut wanted = cfgs.clone();
    wanted[0].ndi_name = "Lab A renamed".into();

    let plan = plan_outputs(&running, &wanted, &nothing_on_air(), false);
    assert_eq!(
        plan.next[0].layers,
        vec!["Lower Third".to_string(), "Bug".to_string()],
        "hardware and content are configured separately"
    );
}

/// The seed round-trip: a node's flags become its file, and its file rebuilds
/// the same specs. This is what makes an existing deployment keep working on
/// the first boot after ADR 0024.
#[test]
fn a_spec_survives_the_trip_through_the_wire_type() {
    let spec = spec_from_config(&ndi("a", "Lab A", "ga", 8621), None);
    let back = spec_from_config(&config_from_spec(&spec), Some(&spec));
    assert_eq!(back.id, spec.id);
    assert_eq!(back.backend, spec.backend);
    assert_eq!(back.ndi_name, spec.ndi_name);
    assert_eq!(back.group, spec.group);
    assert_eq!(back.preview_port, spec.preview_port);
    assert_eq!(back.fps, spec.fps);
    assert_eq!((back.width, back.height), (spec.width, spec.height));
}
