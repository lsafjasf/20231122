use downmix_core::model::Revision;
use downmix_core::recipe::build_params;

#[test]
fn builds_distinct_legacy_and_current_matrices() {
    let legacy = build_params(Revision::Legacy, 6, 48_000, "F32LE").unwrap();
    let current = build_params(Revision::Current, 6, 48_000, "F32LE").unwrap();
    assert_eq!(legacy.coefficient_matrix.len(), 12);
    assert_eq!(current.coefficient_matrix.len(), 12);
    assert_ne!(
        legacy.coefficient_matrix,
        current.coefficient_matrix,
        "revisions must produce different coefficients"
    );
    // Surround attenuation: current recipe attenuates surrounds (0.7071),
    // legacy recipe keeps them at unity.
    let legacy_rear_left = legacy.coefficient_matrix[4];
    let current_rear_left = current.coefficient_matrix[4];
    assert!((legacy_rear_left - 1.0).abs() < 1e-9);
    assert!((current_rear_left - 0.7071).abs() < 1e-9);
    // Every row targets stereo and every column is accounted for.
    for params in [&legacy, &current] {
        assert_eq!(params.output_channels, 2);
        assert_eq!(params.coefficient_matrix.len(), 2 * params.input_channels);
    }
}

#[test]
fn unsupported_channel_layout_is_an_error() {
    let err = build_params(Revision::Current, 13, 48_000, "F32LE").unwrap_err();
    assert!(err.to_string().contains("no downmix layout"));
}

#[test]
fn mono_input_folds_into_both_outputs() {
    let params = build_params(Revision::Current, 1, 48_000, "F32LE").unwrap();
    assert!(params.coefficient_matrix[0] > 0.0);
    assert!(params.coefficient_matrix[1] > 0.0);
}
