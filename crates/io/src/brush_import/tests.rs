//! Tests for the shared brush mapping.

use super::{Format, ImportedBrush};

#[test]
fn read_any_reads_a_mypaint_file_and_names_the_format() {
    let myb = br#"{"name":"Ink","settings":{"radius_logarithmic":{"base_value":2.0},"dabs_per_actual_radius":{"base_value":0.4},"opaque":{"base_value":0.8},"hardness":{"base_value":0.5}}}"#;
    let imp = super::read_any(myb, "Ink.myb", "Pens").expect("a .myb reads");
    assert_eq!(imp.format, "MyPaint");
    assert_eq!(imp.presets.len(), 1);
    assert_eq!(imp.presets[0].name, "Ink");
    assert_eq!(imp.presets[0].group, "Pens");
}

#[test]
fn unknown_bytes_are_refused_with_the_format_list() {
    let e = super::read_any(b"not a brush", "x.bin", "G").unwrap_err();
    assert!(e.contains(".sut") && e.contains(".brushset"), "{e}");
    assert_eq!(super::detect(b"not a brush", "x.bin"), Format::Unknown);
}

#[test]
fn a_neutral_brush_maps_onto_pressure_size_and_scatter() {
    let brush = ImportedBrush {
        name: "Ink".into(),
        size: 20.0,
        spacing: 0.12,
        opacity: 0.9,
        pressure_size: true,
        min_size: 0.25,
        pressure_opacity: true,
        min_opacity: 0.4,
        scatter: 0.2,
        scatter_count: 2,
        hardness: 0.7,
        roundness: 0.5,
        angle: 30.0,
        ..Default::default()
    };
    let settings = brush.settings("Ink");
    assert!((settings.size - 20.0).abs() < 1e-6);
    assert!((settings.spacing - 0.12).abs() < 1e-6);
    assert!(settings.pressure_size && settings.shape_dynamics.enabled);
    assert!((settings.shape_dynamics.size.minimum - 0.25).abs() < 1e-6);
    assert!(settings.transfer.enabled);
    assert!((settings.transfer.opacity.minimum - 0.4).abs() < 1e-6);
    assert!(settings.scattering.enabled && settings.scattering.count == 2);
    assert!((settings.roundness - 0.5).abs() < 1e-6);
}
