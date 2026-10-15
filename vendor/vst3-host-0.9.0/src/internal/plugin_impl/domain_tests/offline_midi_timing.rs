//! Inspect the actual COM IEventList handed to IAudioProcessor::process, not a
//! separate timing helper. In particular SendMidiAt precedes Process transport.

use super::*;

fn fixture(
    mode: crate::plugin::ProcessMode,
    requirements: Option<u32>,
) -> (PluginImpl, Arc<MockState>, Trace) {
    let trace = Trace::default();
    let state = Arc::new(MockState::default());
    state.capture_origin.store(true, Ordering::Release);
    let mut plugin = plugin_fixture(&trace, &state);
    plugin.runtime.process_context_requirements = requirements;
    plugin.set_process_mode(mode).unwrap();
    plugin.start_processing().unwrap();
    (plugin, state, trace)
}

fn transport(ppq: f64, tempo: f64, sample: i64, playing: bool) -> crate::ProcessTransport {
    crate::ProcessTransport {
        sample_position: sample,
        quarter_note_position: ppq,
        tempo,
        playing,
        time_sig_numerator: 4,
        time_sig_denominator: 4,
    }
}

fn on(note: u8) -> MidiEvent {
    MidiEvent::NoteOn {
        channel: MidiChannel::Ch2,
        note,
        velocity: 93,
    }
}

fn explicit_event(offset: i32) -> PluginEvent {
    PluginEvent {
        bus_index: 0,
        sample_offset: offset,
        ppq_position: 123.75,
        flags: Event_::EventFlags_::kIsLive as u16 | Event_::EventFlags_::kUserReserved1 as u16,
        data: crate::midi::PluginEventData::NoteOn {
            channel: 3,
            pitch: 79,
            tuning: 0.0,
            velocity: 0.625,
            length: 0,
            note_id: 42,
        },
    }
}

fn assert_header(actual: (i32, i32, f64, u16), offset: i32, ppq: f64, flags: u16) {
    assert_eq!((actual.0, actual.1, actual.3), (0, offset, flags));
    assert!(
        (actual.2 - ppq).abs() < 1e-12,
        "{actual:?}; expected PPQ {ppq}"
    );
}

#[test]
fn offline_midi_uses_current_block_transport_after_admission_across_chunks_and_tempo_change() {
    // Event PPQ remains valid even when the processor does not request the optional
    // ProcessContext musical fields. A sample position must never reconstruct PPQ.
    for requirements in [None, Some(0)] {
        let (mut plugin, state, trace) = fixture(crate::ProcessMode::Offline, requirements);
        plugin.send_midi_event_at(on(60), 3).unwrap();
        plugin
            .send_midi_event_at(
                MidiEvent::PolyAftertouch {
                    channel: MidiChannel::Ch2,
                    note: 60,
                    pressure: 71,
                },
                5,
            )
            .unwrap();
        plugin
            .send_midi_event_at(
                MidiEvent::NoteOff {
                    channel: MidiChannel::Ch2,
                    note: 60,
                    velocity: 27,
                },
                29,
            )
            .unwrap();
        let explicit = explicit_event(20);
        plugin.send_plugin_event(explicit.clone()).unwrap();
        plugin.set_parameter_at(77, 0.375, 21).unwrap();
        // Exactly the helper ordering: MIDI first, then transport on Process.
        plugin
            .set_process_transport(transport(9.25, 150.0, 7_000, true))
            .unwrap();
        trace.clear();
        plugin
            .process(&mut AudioBuffers::new(1, 1, 32, 48_000.0))
            .unwrap();
        // The configured maximum is 16: both SDK calls must see rebased offsets,
        // an advanced musical origin and the unchanged FIFO/payload kinds.
        let calls = state.origin_calls.lock().unwrap();
        assert_eq!(calls.len(), 2);
        assert_eq!(
            calls[0].events,
            [
                (kNoteOnEvent as u16, 3, 60),
                (kPolyPressureEvent as u16, 5, -1)
            ]
        );
        assert_eq!(
            calls[1].events,
            [(kNoteOffEvent as u16, 13, 60), (kNoteOnEvent as u16, 4, 79)]
        );
        let ppq = |offset: f64| 9.25 + offset / 48_000.0 * (150.0 / 60.0);
        assert_header(calls[0].event_headers[0], 3, ppq(3.0), 0);
        assert_header(calls[0].event_headers[1], 5, ppq(5.0), 0);
        assert_header(calls[1].event_headers[0], 13, ppq(29.0), 0);
        assert_header(
            calls[1].event_headers[1],
            4,
            explicit.ppq_position,
            explicit.flags,
        );
        drop(calls);
        assert_eq!(
            *state.processed_parameters.lock().unwrap(),
            [vec![], vec![(77, 5, 0.375)]]
        );
        assert_eq!(
            trace.methods(),
            [("component", "process"), ("component", "process")]
        );

        // Non-contiguous sample and PPQ positions plus a different tempo invalidate
        // any attempt to stamp the event at admission using the preceding block.
        plugin.send_midi_event_at(on(62), 7).unwrap();
        plugin
            .set_process_transport(transport(12.75, 81.0, -177, true))
            .unwrap();
        plugin
            .process(&mut AudioBuffers::new(1, 1, 16, 48_000.0))
            .unwrap();
        let calls = state.origin_calls.lock().unwrap();
        assert_header(
            calls[2].event_headers[0],
            7,
            12.75 + 7.0 / 48_000.0 * (81.0 / 60.0),
            0,
        );
        drop(calls);

        // Without a fresh update the existing playing transport advances. It does
        // so independently of whether ProcessContext music was requested.
        plugin.send_midi_event_at(on(64), 2).unwrap();
        plugin
            .process(&mut AudioBuffers::new(1, 1, 16, 48_000.0))
            .unwrap();
        assert_header(
            state.origin_calls.lock().unwrap()[3].event_headers[0],
            2,
            12.75 + 18.0 / 48_000.0 * (81.0 / 60.0),
            0,
        );
    }
}

#[test]
fn offline_midi_stopped_and_zero_sample_flush_keep_project_position_and_final_offsets() {
    let (mut plugin, state, _) = fixture(crate::ProcessMode::Offline, Some(0));
    plugin.send_midi_event_at(on(60), 5).unwrap();
    plugin
        .set_process_transport(transport(4.5, 73.0, 1_234, false))
        .unwrap();
    plugin
        .process(&mut AudioBuffers::new(1, 1, 16, 48_000.0))
        .unwrap();
    plugin.send_midi_event_at(on(62), 9_000).unwrap();
    plugin
        .process(&mut AudioBuffers::new(1, 1, 0, 48_000.0))
        .unwrap();
    let calls = state.origin_calls.lock().unwrap();
    assert_eq!(calls[0].frames, 16);
    assert_eq!(calls[1].frames, 0);
    assert_header(calls[0].event_headers[0], 5, 4.5, 0);
    assert_header(calls[1].event_headers[0], 0, 4.5, 0);
}

#[test]
fn offline_midi_without_explicit_transport_integrates_tempo_from_the_default_origin() {
    let (mut plugin, state, _) = fixture(crate::ProcessMode::Offline, Some(0));
    plugin.send_midi_event_at(on(60), 4).unwrap();
    plugin
        .process(&mut AudioBuffers::new(1, 1, 16, 48_000.0))
        .unwrap();
    plugin.set_tempo(60.0).unwrap();
    plugin.send_midi_event_at(on(62), 5).unwrap();
    plugin
        .process(&mut AudioBuffers::new(1, 1, 16, 48_000.0))
        .unwrap();
    assert!(plugin.runtime.process_transport.is_none());
    let calls = state.origin_calls.lock().unwrap();
    assert_header(calls[0].event_headers[0], 4, 4.0 / 48_000.0 * 2.0, 0);
    assert_header(
        calls[1].event_headers[0],
        5,
        (16.0 * 2.0 + 5.0) / 48_000.0,
        0,
    );
}

#[test]
fn realtime_and_prefetch_midi_keep_live_metadata_and_explicit_event_headers() {
    for mode in [crate::ProcessMode::Realtime, crate::ProcessMode::Prefetch] {
        let (mut plugin, state, _) = fixture(mode, None);
        plugin.send_midi_event_at(on(60), 5).unwrap();
        plugin
            .send_midi_event_at(
                MidiEvent::PolyAftertouch {
                    channel: MidiChannel::Ch2,
                    note: 60,
                    pressure: 71,
                },
                7,
            )
            .unwrap();
        plugin
            .send_midi_event_at(
                MidiEvent::NoteOff {
                    channel: MidiChannel::Ch2,
                    note: 60,
                    velocity: 27,
                },
                11,
            )
            .unwrap();
        let explicit = explicit_event(8);
        plugin.send_plugin_event(explicit.clone()).unwrap();
        plugin
            .set_process_transport(transport(8.5, 135.0, 32_100, true))
            .unwrap();
        plugin
            .process(&mut AudioBuffers::new(1, 1, 16, 48_000.0))
            .unwrap();
        let calls = state.origin_calls.lock().unwrap();
        assert_eq!(calls[0].event_headers.len(), 4);
        for (index, offset) in [5, 7, 11].into_iter().enumerate() {
            assert_header(
                calls[0].event_headers[index],
                offset,
                0.0,
                Event_::EventFlags_::kIsLive as u16,
            );
        }
        assert_header(
            calls[0].event_headers[3],
            8,
            explicit.ppq_position,
            explicit.flags,
        );
    }
}
