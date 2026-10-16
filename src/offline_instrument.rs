//! Thread-affine synchronous owner for one fresh isolated VST3 instrument.
//!
//! Isolated bounce preparation deliberately bypasses the realtime bridge while retaining
//! its backend/helper contracts. The experimental App occurrence renderer uses a strict clone.

#[cfg(feature = "vst3")]
use crate::instrument_bounce::BounceAudioBus;
use crate::instrument_bounce::{BouncePluginMetadata, CapturedInstrumentBounce};

#[cfg(feature = "vst3")]
#[path = "offline_effect_capture.rs"]
mod effect_capture;

#[cfg(all(
    feature = "vst3",
    any(target_os = "linux", windows),
    feature = "experimental-fresh-helper"
))]
#[path = "bounce_selected_outputs.rs"]
mod bounce_selected_outputs;

#[cfg(all(test, feature = "vst3", unix))]
#[path = "offline_instrument_protocol_tests.rs"]
mod protocol_tests;

#[cfg(feature = "vst3")]
use {
    super::{BounceCaptureFence, PluginBackend, PluginPrepareConfig, Vst3Backend},
    crate::instrument_bounce::{
        BOUNCE_PREPARED_MAXIMUM, BounceCaptureError, BouncePluginState, MAX_BOUNCE_STATE_BYTES,
    },
    std::collections::BTreeMap,
};

// Refusal-only, scalar diagnostics for unpublished experimental fresh candidates.
// These records never query or change the plugin and never alter admission.
#[cfg(all(
    any(target_os = "linux", windows),
    feature = "experimental-fresh-helper"
))]
mod fresh_topology_diagnostic {
    use crate::instrument_bounce::BouncePluginMetadata;
    use serde::Serialize;
    use vst3_host::audio::AudioBusLayout;

    const MAX_BUSES: usize = 16;
    const MAX_ERROR_CHARS: usize = 256;
    const MARKER: &str = "\nFreshBounceTopologyRefusalV1=";

    #[derive(Serialize)]
    #[serde(rename_all = "snake_case")]
    pub(super) enum Phase {
        ProposedPreflight,
        FinalPreStart,
        PostStart,
    }

    #[derive(Serialize)]
    struct BoundedList<T> {
        total_count: usize,
        truncated: bool,
        entries: Vec<T>,
    }

    impl<T> BoundedList<T> {
        fn from_slice<S>(items: &[S], project: impl Fn(&S) -> T) -> Self {
            Self {
                total_count: items.len(),
                truncated: items.len() > MAX_BUSES,
                // Apply the cap before materializing or cloning any descriptor.
                entries: items.iter().take(MAX_BUSES).map(project).collect(),
            }
        }
    }

    #[derive(Serialize)]
    struct CheckedBus {
        channel_count: usize,
        is_main: bool,
        active: Option<bool>,
    }

    #[derive(Serialize)]
    struct PreparedBus {
        channel_count: usize,
        active: bool,
    }

    #[derive(Serialize)]
    struct Buses<T> {
        inputs: BoundedList<T>,
        outputs: BoundedList<T>,
    }

    impl Buses<CheckedBus> {
        fn checked(metadata: &BouncePluginMetadata) -> Self {
            let project = |bus: &crate::instrument_bounce::BounceAudioBus| CheckedBus {
                channel_count: bus.channel_count,
                is_main: bus.is_main,
                active: bus.active,
            };
            Self {
                inputs: BoundedList::from_slice(&metadata.input_buses, project),
                outputs: BoundedList::from_slice(&metadata.output_buses, project),
            }
        }
    }

    impl Buses<PreparedBus> {
        fn prepared(layout: &AudioBusLayout) -> Self {
            let project = |bus: &vst3_host::audio::AudioBusConfig| PreparedBus {
                channel_count: bus.channel_count,
                active: bus.active,
            };
            Self {
                inputs: BoundedList::from_slice(&layout.inputs, project),
                outputs: BoundedList::from_slice(&layout.outputs, project),
            }
        }
    }

    #[derive(Serialize)]
    struct Refusal {
        schema: u32,
        kind: &'static str,
        validation_phase: Phase,
        validator_error_excerpt: String,
        validator_error_truncated: bool,
        raw_checked: Buses<CheckedBus>,
        prepared_observation_phase: Option<&'static str>,
        prepared: Option<Buses<PreparedBus>>,
        proposal: Option<Buses<CheckedBus>>,
        expected_before_start: Option<Buses<CheckedBus>>,
    }

    pub(super) fn error(
        original: String,
        phase: Phase,
        actual: &BouncePluginMetadata,
        prepared: Option<&AudioBusLayout>,
        proposal: Option<&BouncePluginMetadata>,
        prior: Option<&BouncePluginMetadata>,
    ) -> String {
        let record = Refusal {
            schema: 1,
            kind: "fresh_bounce_topology_refusal",
            validation_phase: phase,
            validator_error_excerpt: original.chars().take(MAX_ERROR_CHARS).collect(),
            validator_error_truncated: original.chars().nth(MAX_ERROR_CHARS).is_some(),
            raw_checked: Buses::checked(actual),
            prepared_observation_phase: prepared.map(|_| "initial_preflight_read"),
            prepared: prepared.map(Buses::prepared),
            proposal: proposal.map(Buses::checked),
            expected_before_start: prior.map(Buses::checked),
        };
        // Preserve the original rejection even if diagnostic formatting fails.
        match serde_json::to_string(&record) {
            Ok(json) => format!("{original}{MARKER}{json}"),
            Err(_) => original,
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::instrument_bounce::BounceAudioBus;
        use serde_json::Value;
        use vst3_host::audio::AudioBusConfig;

        fn metadata() -> BouncePluginMetadata {
            BouncePluginMetadata {
                class_uid: "ABCDEF019182FAEB566D624153675854".into(),
                name: "not emitted".into(),
                version: "not emitted".into(),
                category: "Instrument".into(),
                event_input_buses: vec![(16, true, Some(true))],
                event_output_buses: vec![],
                input_buses: vec![BounceAudioBus::new(2, false, Some(true))],
                output_buses: vec![
                    BounceAudioBus::new(2, true, Some(true)),
                    BounceAudioBus::new(2, true, None),
                    BounceAudioBus::new(2, true, None),
                ],
                latency_samples: 0,
                tail_samples: 96_000,
            }
        }

        fn parse(message: &str, prefix: &str) -> Value {
            let (actual_prefix, json) = message.split_once(MARKER).unwrap();
            assert_eq!(actual_prefix, prefix);
            assert!(!json.contains("not emitted"));
            serde_json::from_str(json).unwrap()
        }

        #[test]
        fn refusal_preserves_raw_roles_unknowns_and_separate_proposal() {
            let mut actual = metadata();
            // Roles are supported independently of participation. This refusal now
            // uses an explicit channel bound while retaining the observed roles.
            actual.output_buses[2].channel_count = 3;
            let mut proposal = actual.clone();
            for bus in &mut proposal.output_buses[1..] {
                bus.active = Some(false);
            }
            let original = proposal.validate(48_000).unwrap_err();
            let prepared = AudioBusLayout {
                inputs: vec![AudioBusConfig {
                    channel_count: 2,
                    active: true,
                }],
                outputs: vec![
                    AudioBusConfig {
                        channel_count: 2,
                        active: true,
                    },
                    AudioBusConfig {
                        channel_count: 2,
                        active: false,
                    },
                    AudioBusConfig {
                        channel_count: 3,
                        active: false,
                    },
                ],
            };
            let encoded = error(
                original.clone(),
                Phase::ProposedPreflight,
                &actual,
                Some(&prepared),
                Some(&proposal),
                None,
            );
            let json = parse(&encoded, &original);
            assert_eq!(json["validation_phase"], "proposed_preflight");
            assert_eq!(
                json["raw_checked"]["outputs"]["entries"][2]["channel_count"],
                3
            );
            assert_eq!(
                json["proposal"]["outputs"]["entries"][2]["channel_count"],
                3
            );
            assert_eq!(
                json["prepared"]["outputs"]["entries"][2]["channel_count"],
                3
            );
            assert_eq!(
                json["raw_checked"]["inputs"]["entries"][0]["is_main"],
                false
            );
            for index in [1, 2] {
                assert_eq!(
                    json["raw_checked"]["outputs"]["entries"][index]["is_main"],
                    true
                );
                assert!(json["raw_checked"]["outputs"]["entries"][index]["active"].is_null());
                assert_eq!(
                    json["proposal"]["outputs"]["entries"][index]["active"],
                    false
                );
                assert_eq!(
                    json["prepared"]["outputs"]["entries"][index]["active"],
                    false
                );
            }
            assert_eq!(json["prepared_observation_phase"], "initial_preflight_read");
            assert!(json["expected_before_start"].is_null());
            assert_eq!(actual.output_buses[1].active, None);
            assert_eq!(proposal.validate(48_000).unwrap_err(), original);
        }

        #[test]
        fn post_start_refusal_has_current_and_prior_but_no_invented_prepared_read() {
            let prior = metadata();
            let mut current = prior.clone();
            current.output_buses[0].channel_count = 4;
            let json = parse(
                &error(
                    "original".into(),
                    Phase::PostStart,
                    &current,
                    None,
                    None,
                    Some(&prior),
                ),
                "original",
            );
            assert_eq!(json["validation_phase"], "post_start");
            assert_eq!(
                json["raw_checked"]["outputs"]["entries"][0]["channel_count"],
                4
            );
            assert_eq!(
                json["expected_before_start"]["outputs"]["entries"][0]["channel_count"],
                2
            );
            assert!(json["prepared"].is_null());
            assert!(json["prepared_observation_phase"].is_null());
            assert!(json["proposal"].is_null());
        }

        #[test]
        fn final_pre_start_phase_is_distinct() {
            let actual = metadata();
            let json = parse(
                &error(
                    "rejected".into(),
                    Phase::FinalPreStart,
                    &actual,
                    None,
                    Some(&actual),
                    None,
                ),
                "rejected",
            );
            assert_eq!(json["validation_phase"], "final_pre_start");
            assert!(json["expected_before_start"].is_null());
        }

        #[test]
        fn projection_visits_only_the_bounded_prefix() {
            let values = vec![0_u8; 10_000];
            let visits = std::cell::Cell::new(0);
            let record = BoundedList::from_slice(&values, |_| {
                visits.set(visits.get() + 1);
                7_u8
            });
            assert_eq!(visits.get(), 16);
            assert_eq!(record.entries, vec![7; 16]);
            assert_eq!(record.total_count, 10_000);
            assert!(record.truncated);
        }

        #[test]
        fn construction_caps_lists_before_materialization_without_hiding_counts() {
            let mut actual = metadata();
            actual.output_buses = (0..1_000)
                .map(|index| BounceAudioBus::new(index, index % 2 == 0, None))
                .collect();
            let original = "é".repeat(300);
            let encoded = error(
                original.clone(),
                Phase::ProposedPreflight,
                &actual,
                None,
                Some(&actual),
                Some(&actual),
            );
            let json = parse(&encoded, &original);
            for view in ["raw_checked", "proposal", "expected_before_start"] {
                let outputs = &json[view]["outputs"];
                assert_eq!(outputs["total_count"], 1_000);
                assert_eq!(outputs["truncated"], true);
                assert_eq!(outputs["entries"].as_array().unwrap().len(), 16);
                for index in 0..16 {
                    assert_eq!(outputs["entries"][index]["channel_count"], index);
                    assert!(outputs["entries"][index]["active"].is_null());
                }
            }
            assert_eq!(
                json["validator_error_excerpt"]
                    .as_str()
                    .unwrap()
                    .chars()
                    .count(),
                256
            );
            assert_eq!(json["validator_error_truncated"], true);
            assert!(encoded.len() < 16_384);
        }
    }
}

// The sole initialization exception is tail settlement at the first fresh Start.
// Exhaustive destructuring requires an explicit decision for every future field.
#[cfg(all(
    any(target_os = "linux", windows),
    feature = "experimental-fresh-helper"
))]
mod fresh_start_admission {
    use crate::instrument_bounce::{BounceAudioBus, BouncePluginMetadata};

    #[derive(PartialEq, Eq)]
    struct StableFields<'a> {
        class_uid: &'a str,
        version: &'a str,
        name: &'a str,
        category: &'a str,
        event_input_buses: &'a [(u32, bool, Option<bool>)],
        event_output_buses: &'a [(u32, bool, Option<bool>)],
        input_buses: &'a [BounceAudioBus],
        output_buses: &'a [BounceAudioBus],
        latency_samples: u32,
    }

    impl<'a> From<&'a BouncePluginMetadata> for StableFields<'a> {
        fn from(metadata: &'a BouncePluginMetadata) -> Self {
            let BouncePluginMetadata {
                class_uid,
                version,
                name,
                category,
                event_input_buses,
                event_output_buses,
                input_buses,
                output_buses,
                latency_samples,
                tail_samples: _,
            } = metadata;
            Self {
                class_uid,
                version,
                name,
                category,
                event_input_buses,
                event_output_buses,
                input_buses,
                output_buses,
                latency_samples: *latency_samples,
            }
        }
    }

    pub(super) fn compatible(
        before_start: &BouncePluginMetadata,
        current: &BouncePluginMetadata,
    ) -> bool {
        StableFields::from(before_start) == StableFields::from(current)
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn metadata() -> BouncePluginMetadata {
            BouncePluginMetadata {
                class_uid: "ABCDEF019182FAEB566D624153675854".into(),
                version: "1.3.4".into(),
                name: "Surge XT".into(),
                category: "Instrument|Synth".into(),
                event_input_buses: vec![(16, true, Some(true))],
                event_output_buses: vec![],
                input_buses: vec![BounceAudioBus::new(2, false, Some(true))],
                output_buses: vec![
                    BounceAudioBus::new(2, true, Some(true)),
                    BounceAudioBus::new(2, true, Some(false)),
                    BounceAudioBus::new(2, true, Some(false)),
                ],
                latency_samples: 0,
                tail_samples: 88_200,
            }
        }

        #[test]
        fn matching_and_only_tail_changes_keep_original_operands() {
            for before_tail in [0, 88_200, 96_000, u32::MAX] {
                for after_tail in [0, 88_200, 96_000, u32::MAX] {
                    let mut before = metadata();
                    before.tail_samples = before_tail;
                    let mut current = before.clone();
                    current.tail_samples = after_tail;
                    let originals = (before.clone(), current.clone());
                    assert!(compatible(&before, &current));
                    assert_eq!((before, current), originals);
                }
            }
        }

        #[test]
        fn all_other_nine_fields_refuse_alone_and_with_tail_change() {
            for field in 0..9 {
                for with_tail in [false, true] {
                    let before = metadata();
                    let mut current = before.clone();
                    match field {
                        0 => current.class_uid.replace_range(..1, "1"),
                        1 => current.version.push('x'),
                        2 => current.name.push('x'),
                        3 => current.category.push_str("|Other"),
                        4 => current.event_input_buses[0].0 = 8,
                        5 => current.event_output_buses.push((1, true, Some(true))),
                        6 => current.input_buses[0].is_main = true,
                        7 => current.output_buses[1].is_main = false,
                        8 => current.latency_samples = 1,
                        _ => unreachable!(),
                    }
                    if with_tail {
                        current.tail_samples = 96_000;
                    }
                    let originals = (before.clone(), current.clone());
                    assert!(!compatible(&before, &current), "field {field}");
                    assert_eq!((before, current), originals);
                }
            }
        }

        #[test]
        fn full_values_beyond_diagnostic_caps_still_refuse() {
            let mut before = metadata();
            before.name = "x".repeat(8192);
            let mut current = before.clone();
            current.name.push('y');
            assert!(!compatible(&before, &current));

            before.output_buses.resize(17, before.output_buses[1]);
            current = before.clone();
            current.output_buses[16].active = None;
            assert!(!compatible(&before, &current));

            before.event_input_buses.resize(17, (16, true, Some(true)));
            current = before.clone();
            current.event_input_buses[16].2 = None;
            assert!(!compatible(&before, &current));
        }
    }
}

// Equality-only diagnostics borrow the snapshots already read at the Start fence.
// They neither query the owner nor participate in the admission decision.
#[cfg(all(
    any(target_os = "linux", windows),
    feature = "experimental-fresh-helper"
))]
mod fresh_metadata_mismatch_diagnostic {
    use crate::instrument_bounce::{BounceAudioBus, BouncePluginMetadata};
    use serde::Serialize;
    use std::io::{self, Write};

    const MAX_BUSES: usize = 16;
    const MAX_TEXT_BYTES: usize = 256;
    const MAX_JSON_BYTES: usize = 64 * 1024;
    const ORIGINAL: &str = "Fresh bounce source topology changed at Start before admission";
    const MARKER: &str = "\nFreshBounceMetadataMismatchV1=";

    #[derive(Serialize)]
    struct Text {
        byte_length: usize,
        excerpt: String,
        truncated: bool,
    }

    impl Text {
        fn bounded(value: &str) -> Self {
            let mut end = value.len().min(MAX_TEXT_BYTES);
            while !value.is_char_boundary(end) {
                end -= 1;
            }
            Self {
                byte_length: value.len(),
                excerpt: value[..end].to_owned(),
                truncated: end < value.len(),
            }
        }
    }

    #[derive(Serialize)]
    struct Bus {
        channel_count: usize,
        is_main: bool,
        active: Option<bool>,
    }

    #[derive(Serialize)]
    struct Buses {
        total_count: usize,
        truncated: bool,
        entries: Vec<Bus>,
    }

    impl Buses {
        fn bounded<T>(source: &[T], project: impl Fn(&T) -> Bus) -> Self {
            Self {
                total_count: source.len(),
                truncated: source.len() > MAX_BUSES,
                // Cap before allocating/projecting; keep full-field equality separately.
                entries: source.iter().take(MAX_BUSES).map(project).collect(),
            }
        }
        fn audio(source: &[BounceAudioBus]) -> Self {
            Self::bounded(source, |bus| Bus {
                channel_count: bus.channel_count,
                is_main: bus.is_main,
                active: bus.active,
            })
        }
        fn event(source: &[(u32, bool, Option<bool>)]) -> Self {
            Self::bounded(source, |&(channel_count, is_main, active)| Bus {
                channel_count: channel_count as usize,
                is_main,
                active,
            })
        }
    }

    #[derive(Serialize)]
    struct Metadata {
        class_uid: Text,
        version: Text,
        name: Text,
        category: Text,
        event_input_buses: Buses,
        event_output_buses: Buses,
        input_buses: Buses,
        output_buses: Buses,
        latency_samples: u32,
        tail_samples: u32,
    }

    impl Metadata {
        fn bounded(source: &BouncePluginMetadata) -> Self {
            // Exhaustive destructuring makes a future compared field a compile-time
            // obligation rather than silently omitting it from this evidence schema.
            let BouncePluginMetadata {
                class_uid,
                version,
                name,
                category,
                event_input_buses,
                event_output_buses,
                input_buses,
                output_buses,
                latency_samples,
                tail_samples,
            } = source;
            Self {
                class_uid: Text::bounded(class_uid),
                version: Text::bounded(version),
                name: Text::bounded(name),
                category: Text::bounded(category),
                event_input_buses: Buses::event(event_input_buses),
                event_output_buses: Buses::event(event_output_buses),
                input_buses: Buses::audio(input_buses),
                output_buses: Buses::audio(output_buses),
                latency_samples: *latency_samples,
                tail_samples: *tail_samples,
            }
        }
    }

    #[derive(Serialize)]
    struct Different {
        class_uid: bool,
        version: bool,
        name: bool,
        category: bool,
        event_input_buses: bool,
        event_output_buses: bool,
        input_buses: bool,
        output_buses: bool,
        latency_samples: bool,
        tail_samples: bool,
    }

    impl Different {
        fn full(before: &BouncePluginMetadata, current: &BouncePluginMetadata) -> Self {
            Self {
                class_uid: before.class_uid != current.class_uid,
                version: before.version != current.version,
                name: before.name != current.name,
                category: before.category != current.category,
                event_input_buses: before.event_input_buses != current.event_input_buses,
                event_output_buses: before.event_output_buses != current.event_output_buses,
                input_buses: before.input_buses != current.input_buses,
                output_buses: before.output_buses != current.output_buses,
                latency_samples: before.latency_samples != current.latency_samples,
                tail_samples: before.tail_samples != current.tail_samples,
            }
        }
    }

    #[derive(Serialize)]
    struct Mismatch {
        schema: u32,
        kind: &'static str,
        phase: &'static str,
        before_start: Metadata,
        current_after_start: Metadata,
        different: Different,
    }

    struct BoundedJson {
        bytes: Vec<u8>,
        limit: usize,
    }

    impl Write for BoundedJson {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            let end = self
                .bytes
                .len()
                .checked_add(bytes.len())
                .filter(|end| *end <= self.limit)
                .ok_or_else(|| io::Error::other("metadata diagnostic byte limit"))?;
            // Check the ceiling before reserve/copy. No completed unbounded JSON
            // String is ever allocated and subsequently truncated.
            self.bytes
                .try_reserve_exact(end - self.bytes.len())
                .map_err(|_| io::Error::other("metadata diagnostic allocation"))?;
            self.bytes.extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    pub(super) fn error(before: &BouncePluginMetadata, current: &BouncePluginMetadata) -> String {
        error_with_limit(before, current, MAX_JSON_BYTES)
    }

    fn error_with_limit(
        before: &BouncePluginMetadata,
        current: &BouncePluginMetadata,
        limit: usize,
    ) -> String {
        let record = Mismatch {
            schema: 1,
            kind: "fresh_bounce_metadata_mismatch",
            phase: "post_start",
            before_start: Metadata::bounded(before),
            current_after_start: Metadata::bounded(current),
            different: Different::full(before, current),
        };
        let mut sink = BoundedJson {
            bytes: Vec::new(),
            limit,
        };
        if serde_json::to_writer(&mut sink, &record).is_err() {
            return ORIGINAL.into();
        }
        let Ok(json) = String::from_utf8(sink.bytes) else {
            return ORIGINAL.into();
        };
        let mut result = ORIGINAL.to_owned();
        if result.try_reserve_exact(MARKER.len() + json.len()).is_err() {
            return result;
        }
        result.push_str(MARKER);
        result.push_str(&json);
        result
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use serde_json::Value;
        use std::cell::Cell;

        fn metadata() -> BouncePluginMetadata {
            BouncePluginMetadata {
                class_uid: "ABCDEF019182FAEB566D624153675854".into(),
                name: "Surge XT".into(),
                version: "1.3.4".into(),
                category: "Instrument|Synth".into(),
                event_input_buses: vec![(16, true, Some(true))],
                event_output_buses: vec![],
                input_buses: vec![BounceAudioBus::new(2, false, Some(true))],
                output_buses: vec![
                    BounceAudioBus::new(2, true, Some(true)),
                    BounceAudioBus::new(2, true, Some(false)),
                    BounceAudioBus::new(2, true, Some(false)),
                ],
                latency_samples: 0,
                tail_samples: 96_000,
            }
        }
        fn parse(before: &BouncePluginMetadata, current: &BouncePluginMetadata) -> Value {
            let result = error(before, current);
            let (prefix, json) = result.split_once(MARKER).unwrap();
            assert_eq!(prefix, ORIGINAL);
            assert_eq!(result.matches(MARKER).count(), 1);
            assert!(json.len() <= MAX_JSON_BYTES);
            serde_json::from_str(json).unwrap()
        }
        fn assert_flags(json: &Value, expected: &[&str]) {
            let flags = json["different"].as_object().unwrap();
            assert_eq!(flags.len(), 10);
            for (field, value) in flags {
                assert_eq!(
                    value.as_bool(),
                    Some(expected.contains(&field.as_str())),
                    "{field}"
                );
            }
        }

        #[test]
        fn each_complete_compared_field_has_its_own_exact_flag() {
            let before = metadata();
            assert_flags(&parse(&before, &before), &[]);
            for field in [
                "class_uid",
                "version",
                "name",
                "category",
                "input_buses",
                "output_buses",
                "event_input_buses",
                "event_output_buses",
                "latency_samples",
                "tail_samples",
            ] {
                let mut current = before.clone();
                match field {
                    "class_uid" => current.class_uid.push('x'),
                    "version" => current.version.push('x'),
                    "name" => current.name.push('x'),
                    "category" => current.category.push('x'),
                    "input_buses" => current.input_buses[0].is_main = true,
                    "output_buses" => current.output_buses[1].channel_count = 1,
                    "event_input_buses" => current.event_input_buses[0].0 = 1,
                    "event_output_buses" => current.event_output_buses.push((1, false, None)),
                    "latency_samples" => current.latency_samples = 1,
                    "tail_samples" => current.tail_samples = u32::MAX,
                    _ => unreachable!(),
                }
                let json = parse(&before, &current);
                assert_eq!(json["schema"], 1);
                assert_eq!(json["kind"], "fresh_bounce_metadata_mismatch");
                assert_eq!(json["phase"], "post_start");
                assert_flags(&json, &[field]);
            }
        }

        #[test]
        fn both_operands_preserve_identity_numbers_and_all_bus_evidence() {
            let mut before = metadata();
            before.event_output_buses = vec![(1, false, None), (2, true, Some(false))];
            before.input_buses[0].active = None;
            let mut current = before.clone();
            current.input_buses[0].active = Some(false);
            current.output_buses.swap(0, 1);
            current.event_input_buses.clear();
            current.event_output_buses[0] = (16, true, Some(true));
            current.latency_samples = u32::MAX;
            current.tail_samples = 0;
            let json = parse(&before, &current);
            for (key, source) in [("before_start", &before), ("current_after_start", &current)] {
                let view = &json[key];
                for (field, text) in [
                    ("class_uid", &source.class_uid),
                    ("version", &source.version),
                    ("name", &source.name),
                    ("category", &source.category),
                ] {
                    assert_eq!(view[field]["byte_length"], text.len());
                    assert_eq!(view[field]["excerpt"], text.as_str());
                    assert_eq!(view[field]["truncated"], false);
                }
                assert_eq!(view["latency_samples"], source.latency_samples);
                assert_eq!(view["tail_samples"], source.tail_samples);
                for (field, list) in [
                    ("input_buses", &source.input_buses),
                    ("output_buses", &source.output_buses),
                ] {
                    let entries = view[field]["entries"].as_array().unwrap();
                    assert_eq!(view[field]["total_count"], list.len());
                    assert_eq!(view[field]["truncated"], false);
                    assert_eq!(entries.len(), list.len());
                    for (entry, bus) in entries.iter().zip(list) {
                        assert_eq!(entry["channel_count"], bus.channel_count);
                        assert_eq!(entry["is_main"], bus.is_main);
                        assert_eq!(entry["active"], serde_json::to_value(bus.active).unwrap());
                    }
                }
                for (field, list) in [
                    ("event_input_buses", &source.event_input_buses),
                    ("event_output_buses", &source.event_output_buses),
                ] {
                    let entries = view[field]["entries"].as_array().unwrap();
                    assert_eq!(view[field]["total_count"], list.len());
                    assert_eq!(view[field]["truncated"], false);
                    assert_eq!(entries.len(), list.len());
                    for (entry, &(channels, main, active)) in entries.iter().zip(list) {
                        assert_eq!(entry["channel_count"], channels);
                        assert_eq!(entry["is_main"], main);
                        assert_eq!(entry["active"], serde_json::to_value(active).unwrap());
                    }
                }
            }
            assert_flags(
                &json,
                &[
                    "input_buses",
                    "output_buses",
                    "event_input_buses",
                    "event_output_buses",
                    "latency_samples",
                    "tail_samples",
                ],
            );
        }

        #[test]
        fn utf8_excerpt_is_byte_bounded_and_same_long_prefix_is_not_equality() {
            for text in [
                "x".repeat(255) + "é",
                "😀".repeat(65),
                "\0\"\\\n".repeat(100),
            ] {
                let bounded = Text::bounded(&text);
                assert_eq!(bounded.byte_length, text.len());
                assert!(bounded.excerpt.len() <= MAX_TEXT_BYTES);
                assert!(text.starts_with(&bounded.excerpt));
                assert!(bounded.truncated);
            }
            let exact = Text::bounded(&"é".repeat(128));
            assert_eq!(exact.excerpt.len(), 256);
            assert!(!exact.truncated);
            let mut before = metadata();
            before.name = "same".repeat(2_048) + "before";
            let mut current = before.clone();
            current.name = "same".repeat(2_048) + "after!";
            let json = parse(&before, &current);
            assert_eq!(
                json["before_start"]["name"],
                json["current_after_start"]["name"]
            );
            assert_eq!(json["before_start"]["name"]["truncated"], true);
            assert_flags(&json, &["name"]);
        }

        #[test]
        fn capped_lists_keep_counts_and_differences_beyond_visible_entries() {
            let mut before = metadata();
            before.input_buses = vec![BounceAudioBus::new(2, false, None); 1_000];
            before.event_output_buses = vec![(2, false, None); 1_000];
            let mut current = before.clone();
            current.input_buses[900].active = Some(false);
            current.event_output_buses[900].0 = 3;
            let json = parse(&before, &current);
            for field in ["input_buses", "event_output_buses"] {
                assert_eq!(
                    json["before_start"][field],
                    json["current_after_start"][field]
                );
                let view = &json["before_start"][field];
                assert_eq!(view["total_count"], 1_000);
                assert_eq!(view["truncated"], true);
                assert_eq!(view["entries"].as_array().unwrap().len(), MAX_BUSES);
            }
            assert_flags(&json, &["input_buses", "event_output_buses"]);
            let visits = Cell::new(0);
            let bounded = Buses::bounded(&before.input_buses, |_| {
                visits.set(visits.get() + 1);
                Bus {
                    channel_count: 2,
                    is_main: false,
                    active: None,
                }
            });
            assert_eq!(visits.get(), MAX_BUSES);
            assert_eq!(bounded.entries.len(), MAX_BUSES);
        }

        #[test]
        fn maximum_escaped_views_are_bounded_and_limit_failure_keeps_original() {
            let mut before = metadata();
            before.class_uid = "\0".repeat(1_000);
            before.name = before.class_uid.clone();
            before.version = before.class_uid.clone();
            before.category = before.class_uid.clone();
            before.input_buses = vec![BounceAudioBus::new(usize::MAX, true, Some(false)); 1_000];
            before.output_buses = before.input_buses.clone();
            before.event_input_buses = vec![(u32::MAX, true, Some(false)); 1_000];
            before.event_output_buses = before.event_input_buses.clone();
            let json = parse(&before, &before);
            assert_flags(&json, &[]);
            assert_eq!(error_with_limit(&before, &before, 0), ORIGINAL);
            assert_eq!(error_with_limit(&before, &before, 1), ORIGINAL);
        }

        #[test]
        fn serializer_sink_rejects_growth_before_copy_and_retains_exact_boundary() {
            let mut sink = BoundedJson {
                bytes: Vec::new(),
                limit: 3,
            };
            sink.write_all(b"abc").unwrap();
            assert_eq!(sink.bytes, b"abc");
            assert!(sink.write_all(b"d").is_err());
            assert_eq!(sink.bytes, b"abc");
            sink.flush().unwrap();
        }
    }
}

/// Construct and use on the same background owner thread. The backend is deliberately !Send.
pub struct OfflineInstrument {
    #[cfg(feature = "vst3")]
    _backend: Vst3Backend,
    metadata: BouncePluginMetadata,
    _same_thread: std::marker::PhantomData<std::rc::Rc<()>>,
}

impl OfflineInstrument {
    /// A receipt is mandatory: stale Project.opaque_state or default-state fallback is impossible.
    pub fn from_capture(capture: &CapturedInstrumentBounce) -> Result<Self, String> {
        #[cfg(not(feature = "vst3"))]
        {
            let _ = capture;
            Err("This build does not include VST3 instrument bounce".into())
        }
        #[cfg(feature = "vst3")]
        {
            super::catch_backend(|| {
                Self::load_snapshot(
                    capture.render_spec(),
                    capture.plan().sample_rate(),
                    &capture.state,
                )
            })
        }
    }

    #[cfg(feature = "vst3")]
    fn load_snapshot(
        mut spec: super::PluginLoadSpec,
        sample_rate: u32,
        state: &BouncePluginState,
    ) -> Result<Self, String> {
        let ids: Vec<_> = state.parameter_bases.keys().copied().collect();
        state.validate(&ids, sample_rate)?;
        if spec.initial_state != state.opaque_state
            || spec.class_uid.as_deref() != Some(state.metadata.class_uid.as_str())
        {
            return Err(
                "Fresh bounce load differs from the authoritative captured state/class".into(),
            );
        }
        super::super::prepare_runtime_spec(&mut spec)?;
        let config = PluginPrepareConfig {
            sample_rate: f64::from(sample_rate),
            max_block_frames: BOUNCE_PREPARED_MAXIMUM,
        }
        .validate()?;
        let mut backend = Vst3Backend::load(spec, config)?;
        let identity = backend.plugin.info();
        if identity.uid != state.metadata.class_uid || identity.version != state.metadata.version {
            return Err(
                "The installed VST3 class/version differs from the authoritative captured source"
                    .into(),
            );
        }
        // Fresh load is inactive for processing. Set kOffline before restore/start, never
        // reconfigure the source instance or claim a successful realtime-mode bounce.
        backend
            .plugin
            .set_process_mode(vst3_host::ProcessMode::Offline)
            .map_err(|error| error.to_string())?;
        backend.prepare(config)?;
        if !ids.is_empty() {
            let (_, catalog) =
                super::validate_catalog_snapshot(backend.parameter_catalog_snapshot()?)?;
            for (&id, &value) in &state.parameter_bases {
                if !catalog
                    .iter()
                    .any(|parameter| parameter.id == id && !parameter.read_only)
                {
                    return Err(format!(
                        "Captured parameter base {id} is absent or read-only in the fresh instance"
                    ));
                }
                backend.set_parameter(id, value)?;
            }
        }
        let metadata = backend
            .bounce_metadata(None)
            .map_err(|error| error.to_string())?;
        metadata.validate(sample_rate)?;
        if metadata != state.metadata {
            return Err(
                "Fresh bounce instance metadata differs from the authoritative captured source"
                    .into(),
            );
        }
        backend.input_channels = metadata.participating_input_channels();
        backend.output_channels = metadata.output_buses[0].channel_count;
        Ok(Self {
            _backend: backend,
            metadata,
            _same_thread: std::marker::PhantomData,
        })
    }

    pub fn metadata(&self) -> &BouncePluginMetadata {
        &self.metadata
    }
}

#[cfg(feature = "vst3")]
impl Vst3Backend {
    /// Only the new owner's first prepare may call this, after restore and before Start.
    /// Preflight the entire declaration before the first acknowledgment, then check that
    /// every intermediate operation preserved all descriptors and prepared participation.
    #[cfg(all(
        any(target_os = "linux", windows),
        feature = "experimental-fresh-helper"
    ))]
    pub(super) fn qualify_fresh_bounce_source(
        &mut self,
        sample_rate: u32,
    ) -> Result<BouncePluginMetadata, String> {
        use vst3_host::audio::{BusDirection, MediaType};
        if self.prepared || self.plugin.is_processing() {
            return Err(
                "Fresh bounce source setup requires an unpublished inactive candidate".into(),
            );
        }
        let identity = self.plugin.info();
        if crate::instrument_bounce::kontakt_selected_stereo_bounce_eligible(
            &identity.uid,
            &identity.version,
            &identity.category,
        ) {
            let checked = bounce_selected_outputs::configure_stopped(self, sample_rate)?;
            self.input_channels = checked.participating_input_channels();
            self.output_channels = checked.output_buses[0].channel_count;
            return Ok(checked);
        }
        let mut actual = self
            .bounce_metadata(None)
            .map_err(|error| error.to_string())?;
        let prepared = self
            .plugin
            .audio_bus_layout()
            .map_err(|error| error.to_string())?;
        let mut proposed = actual.clone();
        let mut acknowledge = Vec::new();
        for (direction, declared, projected, participating) in [
            (
                BusDirection::Input,
                &actual.input_buses,
                &mut proposed.input_buses,
                &prepared.inputs,
            ),
            (
                BusDirection::Output,
                &actual.output_buses,
                &mut proposed.output_buses,
                &prepared.outputs,
            ),
        ] {
            if declared.len() != participating.len() {
                return Err("Fresh bounce source actual/prepared bus count differs".into());
            }
            for (index, ((bus, proposal), storage)) in declared
                .iter()
                .zip(projected.iter_mut())
                .zip(participating)
                .enumerate()
            {
                if bus.channel_count != storage.channel_count
                    || bus.active.is_some_and(|active| active != storage.active)
                {
                    return Err("Fresh bounce source actual/prepared participation differs".into());
                }
                if bus.active.is_none() && !storage.active {
                    // This is a preflight proposal, never a source metadata receipt. Only
                    // a successful owner call plus a checked reread can attest false.
                    proposal.active = Some(false);
                    acknowledge.push((direction, index));
                }
            }
        }
        // Includes actual factory Instrument identity, ALL later buses, event policy and
        // latency. In particular an active aux is rejected rather than disabled to fit.
        proposed.validate(sample_rate).map_err(|error| {
            fresh_topology_diagnostic::error(
                error,
                fresh_topology_diagnostic::Phase::ProposedPreflight,
                &actual,
                Some(&prepared),
                Some(&proposed),
                None,
            )
        })?;
        for (direction, index) in acknowledge {
            self.plugin
                .set_bus_active(MediaType::Audio, direction, index as i32, false)
                .map_err(|error| error.to_string())?;
            match direction {
                BusDirection::Input => actual.input_buses[index].active = Some(false),
                BusDirection::Output => actual.output_buses[index].active = Some(false),
            }
            let checked = self
                .bounce_metadata(None)
                .map_err(|error| error.to_string())?;
            let storage = self
                .plugin
                .audio_bus_layout()
                .map_err(|error| error.to_string())?;
            if checked != actual || storage != prepared {
                return Err(
                    "Fresh bounce source topology changed during inactive-bus acknowledgment"
                        .into(),
                );
            }
        }
        let checked = self
            .bounce_metadata(None)
            .map_err(|error| error.to_string())?;
        checked.validate(sample_rate).map_err(|error| {
            fresh_topology_diagnostic::error(
                error,
                fresh_topology_diagnostic::Phase::FinalPreStart,
                &checked,
                Some(&prepared),
                Some(&proposed),
                None,
            )
        })?;
        if checked != proposed {
            return Err("Fresh bounce source topology changed before admission".into());
        }
        self.input_channels = checked.participating_input_channels();
        self.output_channels = checked.output_buses[0].channel_count;
        Ok(checked)
    }

    #[cfg(all(
        any(target_os = "linux", windows),
        feature = "experimental-fresh-helper"
    ))]
    pub(super) fn fresh_topology_post_start_error(
        error: String,
        current: &BouncePluginMetadata,
        before_start: &BouncePluginMetadata,
    ) -> String {
        fresh_topology_diagnostic::error(
            error,
            fresh_topology_diagnostic::Phase::PostStart,
            current,
            None, // No extra prepared-layout query is made after Start.
            None,
            Some(before_start),
        )
    }

    #[cfg(all(
        any(target_os = "linux", windows),
        feature = "experimental-fresh-helper"
    ))]
    pub(super) fn fresh_start_metadata_mismatch_error(
        before_start: &BouncePluginMetadata,
        current: &BouncePluginMetadata,
    ) -> String {
        fresh_metadata_mismatch_diagnostic::error(before_start, current)
    }

    #[cfg(all(
        any(target_os = "linux", windows),
        feature = "experimental-fresh-helper"
    ))]
    pub(super) fn fresh_start_metadata_compatible(
        before_start: &BouncePluginMetadata,
        current: &BouncePluginMetadata,
    ) -> bool {
        fresh_start_admission::compatible(before_start, current)
    }

    pub(super) fn bounce_metadata(
        &self,
        fence: Option<&BounceCaptureFence>,
    ) -> Result<BouncePluginMetadata, BounceCaptureError> {
        let result = (|| {
            let layout = capture_call(fence, || {
                self.plugin
                    .checked_audio_bus_layout()
                    .map_err(classify_metadata_error)
            })?;
            let events = capture_call(fence, || {
                self.plugin
                    .event_bus_layout()
                    .map_err(classify_metadata_error)
            })?;
            let info = self.plugin.info();
            let latency_samples = capture_call(fence, || {
                let value = self
                    .plugin
                    .try_latency_samples()
                    .map_err(classify_metadata_error)?;
                // A returned drift is an observed backend fault, even if cancellation
                // arrives before capture_call's subsequent successful-call checkpoint.
                #[cfg(all(
                    any(target_os = "linux", windows),
                    feature = "experimental-fresh-helper"
                ))]
                self.validate_fresh_latency(value)
                    .map_err(BounceCaptureError::BackendFailed)?;
                Ok(value)
            })?;
            let tail_samples = capture_call(fence, || {
                let value = self
                    .plugin
                    .try_tail_samples()
                    .map_err(classify_metadata_error)?;
                #[cfg(all(
                    any(target_os = "linux", windows),
                    feature = "experimental-fresh-helper"
                ))]
                self.validate_fresh_tail(value)
                    .map_err(BounceCaptureError::BackendFailed)?;
                Ok(value)
            })?;
            Ok(BouncePluginMetadata {
                class_uid: info.uid.clone(),
                version: info.version.clone(),
                name: info.name.clone(),
                category: info.category.clone(),
                event_input_buses: events
                    .inputs
                    .iter()
                    .map(|bus| (bus.channel_count, bus.is_main, bus.active))
                    .collect(),
                event_output_buses: events
                    .outputs
                    .iter()
                    .map(|bus| (bus.channel_count, bus.is_main, bus.active))
                    .collect(),
                input_buses: layout
                    .inputs
                    .iter()
                    .map(|bus| BounceAudioBus::new(bus.channel_count, bus.is_main, bus.active))
                    .collect(),
                output_buses: layout
                    .outputs
                    .iter()
                    .map(|bus| BounceAudioBus::new(bus.channel_count, bus.is_main, bus.active))
                    .collect(),
                latency_samples,
                tail_samples,
            })
        })();
        #[cfg(test)]
        self.observation.metadata(
            "Vst3Backend actual bounce_metadata",
            result.as_ref().map_err(|e| match e {
                BounceCaptureError::Rejected(message)
                | BounceCaptureError::BackendFailed(message) => message.as_str(),
            }),
        );
        result
    }

    pub(super) fn capture_bounce_state(
        &mut self,
        parameter_ids: &[u32],
        fence: &BounceCaptureFence,
    ) -> Result<BouncePluginState, BounceCaptureError> {
        self.capture_bounce_state_inner(parameter_ids, fence, false)
    }

    fn capture_bounce_state_inner(
        &mut self,
        parameter_ids: &[u32],
        fence: &BounceCaptureFence,
        automated: bool,
    ) -> Result<BouncePluginState, BounceCaptureError> {
        use BounceCaptureError::{BackendFailed, Rejected};
        fence.checkpoint()?;
        if !self.prepared || self.last_transport.playing {
            return Err(Rejected(
                "Instrument bounce source has not reached stopped processing".into(),
            ));
        }
        let before = capture_call(Some(fence), || {
            self.plugin
                .isolated_editor(vst3_host::IsolatedEditorCommand::Query)
                .map_err(|error| BackendFailed(error.to_string()))
        })?;
        if before.open {
            return Err(Rejected(
                "Close the source native editor before instrument bounce".into(),
            ));
        }
        let revision = capture_call(Some(fence), || {
            self.plugin
                .native_dirty_revision()
                .map_err(|error| BackendFailed(error.to_string()))
        })?;
        let metadata = self.bounce_metadata(Some(fence))?;
        metadata
            .validate(self.plugin.sample_rate() as u32)
            .map_err(Rejected)?;
        // Production SaveState has a zero-sample parameter flush. No Close, positive Process,
        // LoadState, transport reset or shared native publication is performed here.
        let opaque_state = capture_call(Some(fence), || self.save_state().map_err(BackendFailed))?;
        if opaque_state.is_empty() || opaque_state.len() > MAX_BOUNCE_STATE_BYTES {
            return Err(Rejected(
                "Instrument bounce requires nonempty authoritative state within 64 MiB".into(),
            ));
        }
        let mut parameter_bases = BTreeMap::new();
        if !parameter_ids.is_empty() {
            let catalog = if automated {
                self.private_generator_automation_catalog(fence)?.parameters
            } else {
                super::validate_catalog_snapshot(capture_call(Some(fence), || {
                    self.parameter_catalog_snapshot().map_err(BackendFailed)
                })?)
                .map_err(Rejected)?
                .1
            };
            for &id in parameter_ids {
                if !catalog
                    .iter()
                    .any(|parameter| parameter.id == id && !parameter.read_only)
                {
                    return Err(Rejected(format!(
                        "Stored parameter base {id} is missing or read-only in current metadata"
                    )));
                }
                let value = capture_call(Some(fence), || {
                    self.get_parameter(id).map_err(BackendFailed)
                })?;
                if !value.is_finite() || !(0.0..=1.0).contains(&value) {
                    return Err(Rejected(format!(
                        "Stored parameter base {id} has an invalid current value"
                    )));
                }
                if parameter_bases.insert(id, value).is_some() {
                    return Err(Rejected("Duplicate captured parameter key".into()));
                }
            }
        }
        let after = capture_call(Some(fence), || {
            self.plugin
                .isolated_editor(vst3_host::IsolatedEditorCommand::Query)
                .map_err(|error| BackendFailed(error.to_string()))
        })?;
        let after_revision = capture_call(Some(fence), || {
            self.plugin
                .native_dirty_revision()
                .map_err(|error| BackendFailed(error.to_string()))
        })?;
        if after.open
            || before.generation != after.generation
            || revision != after_revision
            || metadata != self.bounce_metadata(Some(fence))?
        {
            return Err(Rejected("Source native generation, state revision or metadata changed during instrument bounce capture".into()));
        }
        fence.checkpoint()?;
        Ok(BouncePluginState {
            metadata,
            opaque_state,
            parameter_bases,
            native_generation: before.generation,
            native_dirty_revision: revision,
        })
    }

    pub(super) fn capture_automated_bounce_state(
        &mut self,
        parameter_ids: &[u32],
        automation_ids: &[u32],
        fence: &BounceCaptureFence,
    ) -> Result<
        (
            BouncePluginState,
            crate::instrument_bounce::GeneratorAutomationCatalog,
        ),
        BounceCaptureError,
    > {
        use BounceCaptureError::Rejected;
        fence.checkpoint()?;
        if !self.prepared || self.last_transport.playing {
            return Err(Rejected(
                "Instrument bounce source has not reached stopped processing".into(),
            ));
        }
        let before = self.private_generator_automation_catalog(fence)?;
        before.validate(automation_ids).map_err(Rejected)?;
        let state = self.capture_bounce_state_inner(parameter_ids, fence, true)?;
        // Read-only meters may move at rest; current values are not catalog
        // schema. The original capture separately brackets requested bases/state.
        if !before.same_schema(&self.private_generator_automation_catalog(fence)?) {
            return Err(Rejected(
                "Whole Generator catalog changed during private capture".into(),
            ));
        }
        fence.checkpoint()?;
        Ok((state, before))
    }

    fn private_generator_automation_catalog(
        &mut self,
        fence: &BounceCaptureFence,
    ) -> Result<crate::instrument_bounce::GeneratorAutomationCatalog, BounceCaptureError> {
        // Keep remote/SDK errors distinct from a healthy catalog exceeding this
        // new contract. The ordinary Instrument entry retains its old API.
        let parameters = capture_call(Some(fence), || {
            self.plugin
                .get_parameters()
                .map_err(|error| BounceCaptureError::BackendFailed(error.to_string()))
        })?;
        generator_automation_catalog(parameters)
    }
}

#[cfg(feature = "vst3")]
fn generator_automation_catalog(
    parameters: Vec<vst3_host::Parameter>,
) -> Result<crate::instrument_bounce::GeneratorAutomationCatalog, BounceCaptureError> {
    use BounceCaptureError::Rejected;
    if parameters.len() > super::MAX_PLUGIN_PARAMETER_CATALOG_ITEMS {
        return Err(Rejected(
            "Whole Generator source catalog exceeds its bounded capture contract".into(),
        ));
    }
    let mut catalog = Vec::with_capacity(parameters.len());
    for parameter in parameters {
        if !parameter.value.is_finite()
            || !(0.0..=1.0).contains(&parameter.value)
            || !parameter.default.is_finite()
            || !(0.0..=1.0).contains(&parameter.default)
        {
            return Err(Rejected(
                "Whole Generator source catalog contains an invalid normalized value".into(),
            ));
        }
        let mut descriptor = super::PluginParameterDescriptor {
            id: parameter.id,
            name: parameter.name,
            unit: parameter.unit,
            current_normalized: parameter.value as f32,
            default_normalized: Some(parameter.default as f32),
            step_count: u32::try_from(parameter.step_count).ok(),
            automatable: parameter.can_automate,
            read_only: parameter.is_read_only,
            bypass: parameter.is_bypass,
        };
        super::bound_parameter_descriptor(&mut descriptor);
        catalog.push(descriptor);
    }
    catalog.sort_by_key(|p| p.id);
    let catalog = crate::instrument_bounce::GeneratorAutomationCatalog {
        revision: super::parameter_catalog_revision(&catalog),
        parameters: catalog,
    };
    catalog.validate(&[]).map_err(Rejected)?;
    Ok(catalog)
}

#[cfg(all(test, feature = "vst3"))]
mod generator_catalog_tests {
    use super::*;
    fn parameter(id: u32) -> vst3_host::Parameter {
        vst3_host::Parameter {
            id,
            name: format!("P{id}"),
            value: 0.25,
            min: 0.0,
            max: 1.0,
            default: 0.5,
            unit: String::new(),
            step_count: 0,
            can_automate: true,
            is_read_only: false,
            is_bypass: false,
            flags: 0,
        }
    }
    #[test]
    fn generator_catalog_capacity_duplicate_and_raw_invalid_values_are_healthy_refusals() {
        for raw in [
            vec![parameter(0); super::super::MAX_PLUGIN_PARAMETER_CATALOG_ITEMS + 1],
            vec![parameter(0), parameter(0)],
            vec![vst3_host::Parameter {
                value: f64::NAN,
                ..parameter(0)
            }],
            vec![vst3_host::Parameter {
                default: 1.01,
                ..parameter(0)
            }],
        ] {
            assert!(matches!(
                generator_automation_catalog(raw),
                Err(BounceCaptureError::Rejected(_))
            ));
        }
    }
    #[test]
    fn generator_catalog_schema_excludes_live_meters_but_keeps_writable_metadata() {
        let first = generator_automation_catalog(vec![parameter(1), parameter(0)]).unwrap();
        let mut changed = first.clone();
        changed.parameters[0].current_normalized = 0.875;
        assert!(first.same_schema(&changed));
        changed.parameters[0].automatable = false;
        assert!(!first.same_schema(&changed));
        assert!(changed.validate(&[0]).is_err());
        assert_eq!(
            first.parameters.iter().map(|p| p.id).collect::<Vec<_>>(),
            [0, 1]
        );
    }
}

/// Preserve the operation's failure before examining post-call cancellation. Once dispatched,
/// IPC is neither retried nor reclassified as harmless merely because its owner cancelled.
#[cfg(feature = "vst3")]
fn capture_call<T>(
    fence: Option<&BounceCaptureFence>,
    operation: impl FnOnce() -> Result<T, BounceCaptureError>,
) -> Result<T, BounceCaptureError> {
    if let Some(fence) = fence {
        fence.checkpoint()?;
    }
    let value = operation()?;
    if let Some(fence) = fence {
        fence.checkpoint()?;
    }
    Ok(value)
}

/// Only explicit bounded admission refusal is harmless. Never infer it from remote text:
/// unknown/old helpers, malformed replies, SDK failures and poisoned state remain faults.
#[cfg(feature = "vst3")]
fn classify_metadata_error(error: vst3_host::Error) -> BounceCaptureError {
    match error {
        vst3_host::Error::MetadataQueryRejected(_) => {
            BounceCaptureError::Rejected(error.to_string())
        }
        _ => BounceCaptureError::BackendFailed(error.to_string()),
    }
}

#[cfg(test)]
mod affinity_tests {
    use super::OfflineInstrument;

    #[test]
    fn offline_owner_is_neither_send_nor_sync() {
        // One candidate impl exists only while the owner excludes each auto trait.
        // Adding Send/Sync makes type inference ambiguous and fails compilation.
        trait AmbiguousIfSend<A> {
            fn check() {}
        }
        impl<T: ?Sized> AmbiguousIfSend<()> for T {}
        impl<T: ?Sized + Send> AmbiguousIfSend<u8> for T {}
        let _ = <OfflineInstrument as AmbiguousIfSend<_>>::check;
        trait AmbiguousIfSync<A> {
            fn check() {}
        }
        impl<T: ?Sized> AmbiguousIfSync<()> for T {}
        impl<T: ?Sized + Sync> AmbiguousIfSync<u8> for T {}
        let _ = <OfflineInstrument as AmbiguousIfSync<_>>::check;
    }
}
