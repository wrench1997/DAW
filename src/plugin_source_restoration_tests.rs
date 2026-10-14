use super::super as runtime;
use super::*;
use crate::instrument_bounce::BounceAudioBus;
use std::{cell::RefCell, collections::BTreeMap};

const UID: &str = "00000000000000000000000000000001";

fn metadata() -> BouncePluginMetadata {
    BouncePluginMetadata {
        class_uid: UID.into(),
        version: "1".into(),
        name: "Restoration fixture".into(),
        category: "Instrument|Synth".into(),
        event_input_buses: vec![(16, true, Some(true))],
        event_output_buses: vec![],
        input_buses: vec![],
        output_buses: vec![BounceAudioBus::new(2, true, Some(true))],
        latency_samples: 0,
        tail_samples: 0,
    }
}

fn expected() -> SourceRestorationExpectation {
    SourceRestorationExpectation::new(
        2,
        41,
        "descriptor-path-id".into(),
        "fixture.vst3".into(),
        48_000,
        metadata(),
        vec![(7, 0.25), (17, 0.75)],
    )
    .unwrap()
}

fn spec() -> PluginLoadSpec {
    let mut spec = PluginLoadSpec::from_descriptor(runtime::PluginDescriptor {
        id: "descriptor-path-id".into(),
        name: "Cached title".into(),
        vendor: "Unattested vendor".into(),
        path: "fixture.vst3".into(),
        format: runtime::PluginFormat::Vst3,
        category: "Instrument".into(),
        is_instrument: true,
        verified: false,
        vst3_metadata: None,
        vst3_catalog: None,
        scan_error: None,
    });
    spec.class_uid = Some(UID.into());
    spec.initial_state = vec![1, 2, 3];
    spec
}

fn config() -> PluginPrepareConfig {
    PluginPrepareConfig {
        sample_rate: 48_000.0,
        max_block_frames: 2048,
    }
}

struct Access {
    catalog: Vec<RestorationParameter>,
    bases: BTreeMap<u32, f64>,
    metadata: BouncePluginMetadata,
    calls: RefCell<Vec<String>>,
    corrupt_earlier_on_last_set: bool,
    getter: Option<f64>,
}

impl Access {
    fn new() -> Self {
        Self {
            catalog: vec![
                RestorationParameter {
                    id: 17,
                    read_only: false,
                },
                RestorationParameter {
                    id: 7,
                    read_only: false,
                },
            ],
            bases: BTreeMap::from([(7, 0.0), (17, 0.0)]),
            metadata: metadata(),
            calls: RefCell::new(vec![]),
            corrupt_earlier_on_last_set: false,
            getter: None,
        }
    }
}

impl RestorationAccess for Access {
    fn restoration_catalog(&self) -> Result<Vec<RestorationParameter>, String> {
        self.calls.borrow_mut().push("catalog".into());
        Ok(self.catalog.clone())
    }
    fn restoration_set(&mut self, id: u32, value: f64) -> Result<(), String> {
        self.calls.borrow_mut().push(format!("set:{id}"));
        self.bases.insert(id, value);
        if self.corrupt_earlier_on_last_set && id == 17 {
            self.bases.insert(7, 0.5);
        }
        Ok(())
    }
    fn restoration_get(&self, id: u32) -> Result<f64, String> {
        self.calls.borrow_mut().push(format!("get:{id}"));
        Ok(self.getter.unwrap_or(self.bases[&id]))
    }
    fn restoration_metadata(&self) -> Result<BouncePluginMetadata, String> {
        self.calls.borrow_mut().push("metadata".into());
        Ok(self.metadata.clone())
    }
}

#[test]
fn restoration_binding_rejects_unsorted_duplicate_nonfinite_and_unbounded_values() {
    for bases in [
        vec![(17, 0.75), (7, 0.25)],
        vec![(7, 0.25), (7, 0.25)],
        vec![(7, f32::NAN)],
        vec![(7, -0.1)],
        vec![(7, 1.1)],
        vec![(7, f32::INFINITY)],
    ] {
        assert!(
            SourceRestorationExpectation::new(
                2,
                41,
                "d".into(),
                "p".into(),
                48_000,
                metadata(),
                bases
            )
            .is_err()
        );
    }
    for case in 0..6 {
        let mut m = metadata();
        let mut session = 2;
        let mut instance = 41;
        let mut rate = 48_000;
        let mut path = PathBuf::from("fixture");
        match case {
            0 => session = 0,
            1 => instance = 0,
            2 => rate = 7999,
            3 => m.class_uid = "not-a-factory-uid".into(),
            4 => m.name = "x".repeat(MAX_BINDING_TEXT_BYTES + 1),
            _ => path = PathBuf::new(),
        }
        assert!(
            SourceRestorationExpectation::new(session, instance, "d".into(), path, rate, m, vec![])
                .is_err()
        );
    }
}

#[test]
fn restoration_requires_exact_session_descriptor_path_class_and_enabled_dry_source() {
    let expected = expected();
    assert!(expected.validate_spec(2, 41, &spec(), config()).is_ok());
    assert!(expected.validate_spec(3, 41, &spec(), config()).is_err());
    assert!(expected.validate_spec(2, 42, &spec(), config()).is_err());
    for case in 0..10 {
        let mut spec = spec();
        match case {
            0 => spec.descriptor.id = UID.into(),
            1 => spec.descriptor.path = "other.vst3".into(),
            2 => spec.class_uid = None,
            3 => spec.class_uid = Some("00000000000000000000000000000002".into()),
            4 => spec.enabled = false,
            5 => spec.bypassed = true,
            6 => spec.wet = 0.5,
            7 => spec.descriptor.is_instrument = false,
            8 => spec.initial_state.clear(),
            _ => spec.descriptor.format = runtime::PluginFormat::Vst2,
        }
        assert!(
            expected.validate_spec(2, 41, &spec, config()).is_err(),
            "case {case}"
        );
    }
}

#[test]
fn restoration_identity_is_actual_factory_not_display_descriptor_or_default_class() {
    let expected = expected();
    let m = metadata();
    assert!(
        expected
            .check_loaded_identity(&m.class_uid, &m.version, &m.name, &m.category)
            .is_ok()
    );
    for field in 0..4 {
        let mut m = metadata();
        match field {
            0 => m.class_uid = "00000000000000000000000000000002".into(),
            1 => m.version.push('2'),
            2 => m.name.push('2'),
            _ => m.category = "Instrument|Other".into(),
        }
        assert!(
            expected
                .check_loaded_identity(&m.class_uid, &m.version, &m.name, &m.category)
                .is_err()
        );
    }
}

#[test]
fn restoration_preflights_the_whole_catalog_before_any_setter() {
    for case in 0..4 {
        let mut access = Access::new();
        match case {
            0 => {
                access.catalog.remove(0);
            }
            1 => access.catalog[0].read_only = true,
            2 => access.catalog.push(RestorationParameter {
                id: 17,
                read_only: false,
            }),
            _ => {
                access.catalog = (0..=MAX_PLUGIN_PARAMETER_CATALOG_ITEMS)
                    .map(|id| RestorationParameter {
                        id: id as u32,
                        read_only: false,
                    })
                    .collect()
            }
        }
        assert!(expected().restore_bases(&mut access).is_err());
        assert_eq!(*access.calls.borrow(), vec!["catalog"]);
    }
}

#[test]
fn restoration_sets_each_base_once_sorted_then_rereads_every_base_and_metadata() {
    let expected = expected();
    let mut access = Access::new();
    expected.restore_bases(&mut access).unwrap();
    expected.verify_after_start(&access).unwrap();
    assert_eq!(
        *access.calls.borrow(),
        vec![
            "catalog", "set:7", "get:7", "set:17", "get:17", "get:7", "get:17", "metadata"
        ]
    );
}

#[test]
fn restoration_rejects_invalid_raw_f64_before_projection_and_exact_bit_mismatch() {
    for value in [
        f64::NAN,
        f64::INFINITY,
        -f64::MIN_POSITIVE,
        1.0000000000000002,
        0.25001,
    ] {
        assert!(check_base(7, 0.25, value).is_err());
    }
    assert!(check_base(7, 0.25, 0.25 + f64::EPSILON).is_ok());
    assert!(check_base(7, 0.0, -0.0).is_err());
    assert!(check_base(7, -0.0, -0.0).is_ok());
}

#[test]
fn restoration_final_read_catches_later_setter_start_and_full_metadata_drift() {
    let expected = expected();
    let mut access = Access::new();
    access.corrupt_earlier_on_last_set = true;
    expected.restore_bases(&mut access).unwrap();
    assert!(expected.verify_after_start(&access).is_err());
    for field in 0..10 {
        let mut access = Access::new();
        expected.restore_bases(&mut access).unwrap();
        match field {
            0 => {
                access.bases.insert(17, 0.5);
            }
            1 => access.metadata.class_uid = "00000000000000000000000000000002".into(),
            2 => access.metadata.version.push('2'),
            3 => access.metadata.name.push('2'),
            4 => access.metadata.category = "Instrument|Other".into(),
            5 => access.metadata.event_input_buses[0].0 = 8,
            6 => access.metadata.output_buses[0] = BounceAudioBus::new(1, true, Some(true)),
            7 => access
                .metadata
                .input_buses
                .push(BounceAudioBus::new(1, true, Some(false))),
            8 => access.metadata.latency_samples = 1,
            _ => access.metadata.tail_samples = 1,
        }
        assert!(
            expected.verify_after_start(&access).is_err(),
            "field {field}"
        );
    }
}

#[test]
fn temporary_initial_timing_accepts_exact_pair_and_none_preserves_default() {
    assert_eq!(check_initial_timing((0, 0), Some((0, 0))).unwrap(), (0, 0));
    assert!(check_initial_timing((1, 0), Some((0, 0))).is_err());
    assert!(check_initial_timing((0, 1), Some((0, 0))).is_err());
    assert_eq!(
        check_initial_timing((1, u32::MAX), None).unwrap(),
        (1, u32::MAX)
    );
}

#[cfg(all(feature = "vst3", unix))]
#[path = "plugin_source_restoration_protocol_tests.rs"]
mod protocol;
