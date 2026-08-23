//! The reference lower third, as Rust.
//!
//! Mirrors `testdata/templates/lower-third-basic.pmgscene.json` and exists so
//! tests and `--selftest` have a realistic document without reading a file —
//! this crate is in the pure set and may not touch the filesystem.

use crate::doc::*;
use pmg_anim::{Ease, Key};

/// A responsive lower third: a content-sized band, a name, and a title that
/// disappears from layout when empty.
pub fn lower_third() -> SceneDoc {
    let name = Node {
        id: "name".into(),
        display_name: Some("Name".into()),
        kind: NodeKind::Text,
        layout: Layout {
            width: Dimension::Content,
            height: Dimension::Content,
            ..Default::default()
        },
        transform: Transform2D::default(),
        style: Style {
            fill: Some(Rgba {
                rgba: [0.93, 0.95, 0.97, 1.0],
                token: None,
            }),
            ..Default::default()
        },
        clip: false,
        visible_if: None,
        content: Some(Content::Text {
            binding: Binding {
                expr: "${name}".into(),
                transform: vec![crate::Transform::Trim, crate::Transform::Ellipsize(40)],
            },
            font: FontSpec {
                family: "IBM Plex Sans".into(),
                weight: 600,
                size: 46.0,
                features: vec!["kern".into(), "liga".into()],
                tracking: 0.0,
                line_height: 1.1,
                token: None,
            },
            valign: Align::default(),
            align: Align::Start,
            max_lines: Some(1),
        }),
        children: vec![],
        unknown: Default::default(),
    };

    let title = Node {
        id: "title".into(),
        display_name: Some("Title".into()),
        kind: NodeKind::Text,
        layout: Layout {
            width: Dimension::Content,
            height: Dimension::Content,
            ..Default::default()
        },
        transform: Transform2D::default(),
        style: Style {
            fill: Some(Rgba {
                rgba: [0.58, 0.64, 0.69, 1.0],
                token: None,
            }),
            ..Default::default()
        },
        clip: false,
        // THE field. False removes this from layout entirely, so the band
        // collapses to one line rather than leaving a gap.
        visible_if: Some(VisibleIf {
            field: "title".into(),
            op: VisibleOp::NonEmpty,
        }),
        content: Some(Content::Text {
            binding: Binding {
                expr: "${title}".into(),
                transform: vec![crate::Transform::Trim, crate::Transform::Ellipsize(56)],
            },
            font: FontSpec {
                family: "IBM Plex Sans".into(),
                weight: 400,
                size: 26.0,
                features: vec!["kern".into()],
                tracking: 0.02,
                line_height: 1.2,
                token: None,
            },
            valign: Align::default(),
            align: Align::Start,
            max_lines: Some(1),
        }),
        children: vec![],
        unknown: Default::default(),
    };

    let band = Node {
        id: "band".into(),
        display_name: Some("Band".into()),
        kind: NodeKind::Box,
        // width: Content is the whole responsive trick. The band sizes to the
        // text it contains, ONCE, when data binds.
        layout: Layout {
            direction: Direction::Column,
            width: Dimension::Content,
            height: Dimension::Content,
            align: Align::Start,
            gap: 2.0,
            padding: Edges {
                top: 18.0,
                right: 34.0,
                bottom: 18.0,
                left: 28.0,
            },
            max_width: Some(Dimension::Px(1200.0)),
            ..Default::default()
        },
        transform: Transform2D {
            anchor: [0.0, 0.5],
            ..Default::default()
        },
        style: Style {
            fill: Some(Rgba {
                rgba: [0.03, 0.05, 0.07, 0.92],
                token: None,
            }),
            radius: 4.0,
            ..Default::default()
        },
        clip: true,
        visible_if: None,
        content: None,
        children: vec![name, title],
        unknown: Default::default(),
    };

    let root = Node {
        id: "root".into(),
        display_name: Some("Root".into()),
        kind: NodeKind::Box,
        layout: Layout {
            direction: Direction::Column,
            justify: Justify::End,
            align: Align::Start,
            width: Dimension::Px(1920.0),
            height: Dimension::Px(1080.0),
            padding: Edges {
                top: 0.0,
                right: 0.0,
                bottom: 120.0,
                left: 140.0,
            },
            ..Default::default()
        },
        transform: Transform2D::default(),
        style: Style::default(),
        clip: false,
        visible_if: None,
        content: None,
        children: vec![band],
        unknown: Default::default(),
    };

    SceneDoc {
        format: "pmg.scene".into(),
        format_version: FormatVersion { major: 1, minor: 0 },
        min_reader: 1,
        id: "01J8ZKLOWERTHIRDBASIC0001".into(),
        name: "Lower Third — Speaker".into(),
        canvas: Canvas {
            width: 1920,
            height: 1080,
            safe: Some(SafeArea {
                action: 0.93,
                title: 0.90,
            }),
        },
        fields: vec![
            Field {
                key: "name".into(),
                ty: FieldType::Text,
                label: "Name".into(),
                required: true,
                max_len: Some(40),
                sample: Some("Pastor Dan Whitfield".into()),
            },
            Field {
                key: "title".into(),
                ty: FieldType::Text,
                label: "Title".into(),
                required: false,
                max_len: None,
                sample: Some("Lead Pastor".into()),
            },
        ],
        fonts: vec![FontRef {
            family: "IBM Plex Sans".into(),
            weight: 600,
            style: Some("normal".into()),
            asset: Some(format!("blake3:{}", "0".repeat(64))),
            system_fallback: false,
        }],
        assets: vec![],
        root,
        timelines: vec![
            Timeline {
                name: "In".into(),
                duration_ms: 800.0,
                on_complete: OnComplete::Hold,
                looping: false,
                // EVERY target is transform.* or style.* — nothing here forces
                // a relayout, which is what makes the reveal free per frame.
                tracks: vec![
                    Track {
                        target: "band/transform.translate.x".into(),
                        blend: Blend::Replace,
                        keys: vec![
                            Key {
                                t: 0.0,
                                v: -900.0,
                                ease: Ease::Cubic([0.16, 1.0, 0.3, 1.0]),
                            },
                            Key {
                                t: 700.0,
                                v: 0.0,
                                ease: Ease::Linear,
                            },
                        ],
                    },
                    Track {
                        target: "title/transform.translate.y".into(),
                        blend: Blend::Replace,
                        keys: vec![
                            Key {
                                t: 0.0,
                                v: 24.0,
                                ease: Ease::Cubic([0.2, 0.0, 0.0, 1.0]),
                            },
                            Key {
                                t: 300.0,
                                v: 24.0,
                                ease: Ease::Cubic([0.2, 0.0, 0.0, 1.0]),
                            },
                            Key {
                                t: 800.0,
                                v: 0.0,
                                ease: Ease::Linear,
                            },
                        ],
                    },
                ],
            },
            Timeline {
                name: "Out".into(),
                duration_ms: 420.0,
                on_complete: OnComplete::Evict,
                looping: false,
                tracks: vec![Track {
                    target: "band/transform.translate.x".into(),
                    blend: Blend::Replace,
                    keys: vec![
                        Key {
                            t: 0.0,
                            v: 0.0,
                            ease: Ease::Cubic([0.7, 0.0, 0.84, 0.0]),
                        },
                        Key {
                            t: 420.0,
                            v: -900.0,
                            ease: Ease::Linear,
                        },
                    ],
                }],
            },
        ],
        theme: None,
        ext: Default::default(),
    }
}
