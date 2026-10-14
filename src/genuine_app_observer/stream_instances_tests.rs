use super::stream_instances::*;

fn header(sequence: u64) -> RecordHeader {
    RecordHeader {
        global_sequence: sequence,
        monotonic_elapsed_ns: sequence * 10,
    }
}
fn initial() -> InstanceIdentity {
    InstanceIdentity {
        ordinal: 1,
        phase: InstancePhase::InitialSource,
        worker_id: Some(7),
        source_instance_id: Some(99),
    }
}
fn strict() -> InstanceIdentity {
    InstanceIdentity {
        ordinal: 2,
        phase: InstancePhase::StrictClone,
        worker_id: None,
        source_instance_id: None,
    }
}
fn undo(ordinal: u64) -> InstanceIdentity {
    InstanceIdentity {
        ordinal,
        phase: InstancePhase::UndoSource,
        worker_id: Some(0),
        source_instance_id: Some(0),
    }
}
fn source() -> SourceIdentity {
    SourceIdentity {
        project_session: 11,
        project_fingerprint: 0,
        timeline_revision: 13,
        epoch: 17,
        endpoint_id: 19,
        channel_id: 23,
        instance_id: 99,
    }
}
fn complete_builder() -> Builder {
    let mut builder = Builder::new();
    builder.add_entry(header(10), initial()).unwrap();
    builder
        .record_initial_capture(header(20), CaptureLabel::Initial, None, 1, 31, source())
        .unwrap();
    builder.add_entry(header(30), strict()).unwrap();
    builder
        .record_strict_consumption(header(40), Some(strict()), 1, 20, source())
        .unwrap();
    builder
}
fn complete() -> Dictionary {
    complete_builder().freeze().unwrap()
}
fn wire() -> Vec<u8> {
    encode(&complete()).unwrap().as_bytes().to_vec()
}
fn put_u32(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}
fn put_u64(bytes: &mut [u8], offset: usize, value: u64) {
    bytes[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
}

#[test]
fn full_supplied_links_roundtrip_every_field_and_strict_none_remains_none() {
    let mut builder = complete_builder();
    builder.add_entry(header(50), undo(3)).unwrap();
    let dictionary = builder.freeze().unwrap();
    let encoded = encode(&dictionary).unwrap();
    assert_eq!(encoded.as_bytes().len(), 424);
    let decoded = decode(encoded.as_bytes()).unwrap();
    assert_eq!(decoded, dictionary);
    assert_eq!(decoded.lookup_exact(strict()).unwrap().identity, strict());
    assert_eq!(decoded.entry(2).unwrap().identity.worker_id, None);
    assert_eq!(decoded.entry(2).unwrap().identity.source_instance_id, None);
    let capture = decoded.capture().unwrap();
    assert_eq!(capture.known_owner_source_ordinal, 1);
    assert_eq!(capture.receipt_identity, 31);
    assert_eq!(capture.source.project_fingerprint, 0);
    assert_eq!(capture.source, source());
    let ConsumptionLink {
        header: event,
        clone_ordinal,
        known_owner_source_ordinal,
        captured_record_sequence,
        source: consumed_stamp,
    } = decoded.consumption().unwrap();
    assert_eq!(
        (
            event,
            clone_ordinal,
            known_owner_source_ordinal,
            captured_record_sequence
        ),
        (header(40), 2, 1, 20)
    );
    assert_eq!(consumed_stamp, source());
    assert!(decoded.links_complete());
    assert!(!decoded.sdk_authority());
}

#[test]
fn independently_packed_literal_wire_decodes_exactly_without_using_the_encoder() {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"NIDICT01");
    for word in [1u32, 2, 64, 3] {
        bytes.extend_from_slice(&word.to_le_bytes());
    }
    bytes.extend_from_slice(&360u64.to_le_bytes());
    bytes.extend_from_slice(&1u64.to_le_bytes());
    for word in [1u32, 0] {
        bytes.extend_from_slice(&word.to_le_bytes());
    }
    for word in [1u64, 7, 1, 99, 10, 100] {
        bytes.extend_from_slice(&word.to_le_bytes());
    }
    bytes.extend_from_slice(&2u64.to_le_bytes());
    for word in [2u32, 0] {
        bytes.extend_from_slice(&word.to_le_bytes());
    }
    for word in [0u64, 0, 0, 0, 30, 300] {
        bytes.extend_from_slice(&word.to_le_bytes());
    }
    for word in [1u32, 0] {
        bytes.extend_from_slice(&word.to_le_bytes());
    }
    for word in [20u64, 200, 1, 31, 11, 0, 13, 17, 19, 23, 99] {
        bytes.extend_from_slice(&word.to_le_bytes());
    }
    for word in [2u32, 0] {
        bytes.extend_from_slice(&word.to_le_bytes());
    }
    for word in [40u64, 400, 2, 1, 20, 11, 0, 13, 17, 19, 23, 99] {
        bytes.extend_from_slice(&word.to_le_bytes());
    }
    assert_eq!(bytes.len(), 360);
    let decoded = decode(&bytes).unwrap();
    assert_eq!(
        decoded.entry(1).unwrap(),
        Entry {
            identity: initial(),
            construction: header(10)
        }
    );
    assert_eq!(
        decoded.entry(2).unwrap(),
        Entry {
            identity: strict(),
            construction: header(30)
        }
    );
    assert_eq!(decoded.capture().unwrap().source, source());
    assert_eq!(decoded.consumption().unwrap().source, source());
    assert_eq!(decoded.capture().unwrap().header, header(20));
    assert_eq!(decoded.consumption().unwrap().header, header(40));
    assert_eq!(encode(&complete()).unwrap().as_bytes(), bytes.as_slice());
}

#[test]
fn optional_worker_and_source_none_are_distinct_from_some_zero_in_wire_and_lookup() {
    let mut builder = Builder::new();
    let a = InstanceIdentity {
        ordinal: 8,
        phase: InstancePhase::InitialSource,
        worker_id: None,
        source_instance_id: None,
    };
    let b = undo(9);
    builder.add_entry(header(1), a).unwrap();
    builder.add_entry(header(3), b).unwrap();
    let dictionary = builder.freeze().unwrap();
    let encoded = encode(&dictionary).unwrap();
    let decoded = decode(encoded.as_bytes()).unwrap();
    assert_eq!(decoded.entry(8).unwrap().identity, a);
    assert_eq!(decoded.entry(9).unwrap().identity, b);
    assert_eq!(&encoded.as_bytes()[32 + 16..32 + 24], &0u64.to_le_bytes());
    assert_eq!(&encoded.as_bytes()[96 + 16..96 + 24], &1u64.to_le_bytes());
    let mut changed = a;
    changed.worker_id = Some(0);
    assert_eq!(decoded.lookup_exact(changed), Err(Error::ChangedTuple));
}

#[test]
fn changing_any_identity_tuple_field_cannot_replace_an_existing_entry() {
    for field in 0..3 {
        let mut builder = Builder::new();
        builder.add_entry(header(1), initial()).unwrap();
        let before = builder.clone();
        let mut changed = initial();
        match field {
            0 => changed.phase = InstancePhase::UndoSource,
            1 => changed.worker_id = None,
            _ => changed.source_instance_id = Some(100),
        }
        assert_eq!(
            builder.add_entry(header(2), changed),
            Err(Error::ChangedTuple)
        );
        assert_eq!(builder, before);
    }
    let dictionary = complete();
    let mut changed = strict();
    changed.ordinal = 100;
    assert_eq!(
        dictionary.lookup_exact(changed),
        Err(Error::UnknownReference)
    );
}

#[test]
fn exact_duplicate_construction_is_rejected_without_advancing_header_or_count() {
    let mut builder = Builder::new();
    builder.add_entry(header(1), initial()).unwrap();
    let before = builder.clone();
    assert_eq!(
        builder.add_entry(header(2), initial()),
        Err(Error::DuplicateOrdinal)
    );
    assert_eq!(builder, before);
    builder.add_entry(header(2), strict()).unwrap();
    assert_eq!(builder.freeze().unwrap().entries().count(), 2);
}

#[test]
fn fixed_four_entry_limit_and_maximum_488_byte_wire_are_real_bounds() {
    let mut builder = complete_builder();
    builder.add_entry(header(50), undo(3)).unwrap();
    builder.add_entry(header(60), undo(4)).unwrap();
    let before = builder.clone();
    assert_eq!(builder.add_entry(header(70), undo(5)), Err(Error::Full));
    assert_eq!(builder, before);
    let dictionary = builder.freeze().unwrap();
    let encoded = encode(&dictionary).unwrap();
    assert_eq!(encoded.as_bytes().len(), MAX_WIRE_BYTES);
    assert_eq!(MAX_WIRE_BYTES, 488);
    assert_eq!(decode(encoded.as_bytes()).unwrap(), dictionary);
}

#[test]
fn entry_zero_ordinals_sequences_duplicate_sequences_and_regressing_timestamps_fail_atomically() {
    let mut builder = Builder::new();
    let mut bad = initial();
    bad.ordinal = 0;
    assert_eq!(builder.add_entry(header(1), bad), Err(Error::ZeroOrdinal));
    assert_eq!(
        builder.add_entry(header(0), initial()),
        Err(Error::ZeroSequence)
    );
    builder.add_entry(header(10), initial()).unwrap();
    let before = builder.clone();
    assert_eq!(
        builder.add_entry(header(10), strict()),
        Err(Error::SequenceOrder)
    );
    assert_eq!(
        builder.add_entry(
            RecordHeader {
                global_sequence: 11,
                monotonic_elapsed_ns: 99
            },
            strict()
        ),
        Err(Error::TimestampOrder)
    );
    assert_eq!(builder, before);
}

#[test]
fn capture_requires_actual_record_instance_none_and_does_not_relabel_the_transcript() {
    let mut builder = Builder::new();
    builder.add_entry(header(1), initial()).unwrap();
    let before = builder.clone();
    assert_eq!(
        builder.record_initial_capture(header(2), CaptureLabel::Final, None, 1, 0, source()),
        Err(Error::WrongCaptureLabel)
    );
    assert_eq!(
        builder.record_initial_capture(
            header(2),
            CaptureLabel::Initial,
            Some(initial()),
            1,
            0,
            source()
        ),
        Err(Error::UnexpectedCaptureInstance)
    );
    assert_eq!(builder, before);
    builder
        .record_initial_capture(header(2), CaptureLabel::Initial, None, 1, 0, source())
        .unwrap();
    let dictionary = builder.freeze().unwrap();
    assert_eq!(dictionary.capture().unwrap().receipt_identity, 0); // exact supplied usize; no pointer authority claimed
    assert!(!dictionary.links_complete());
    assert!(!dictionary.sdk_authority());
}

#[test]
fn capture_attribution_requires_an_initial_entry_and_exact_source_instance_id() {
    let mut builder = Builder::new();
    builder.add_entry(header(1), initial()).unwrap();
    builder.add_entry(header(2), strict()).unwrap();
    let before = builder.clone();
    assert_eq!(
        builder.record_initial_capture(header(3), CaptureLabel::Initial, None, 100, 1, source()),
        Err(Error::UnknownReference)
    );
    assert_eq!(
        builder.record_initial_capture(header(3), CaptureLabel::Initial, None, 2, 1, source()),
        Err(Error::WrongSourcePhase)
    );
    let mut wrong = source();
    wrong.instance_id += 1;
    assert_eq!(
        builder.record_initial_capture(header(3), CaptureLabel::Initial, None, 1, 1, wrong),
        Err(Error::SourceInstanceMismatch)
    );
    assert_eq!(builder, before);
}

#[test]
fn each_nonfingerprint_source_stamp_field_must_be_nonzero_and_fingerprint_zero_is_allowed() {
    for field in 0..6 {
        let mut builder = Builder::new();
        builder.add_entry(header(1), initial()).unwrap();
        let before = builder.clone();
        let mut bad = source();
        match field {
            0 => bad.project_session = 0,
            1 => bad.timeline_revision = 0,
            2 => bad.epoch = 0,
            3 => bad.endpoint_id = 0,
            4 => bad.channel_id = 0,
            _ => bad.instance_id = 0,
        }
        assert_eq!(
            builder.record_initial_capture(header(2), CaptureLabel::Initial, None, 1, 1, bad),
            Err(Error::SourceStamp)
        );
        assert_eq!(builder, before);
    }
    assert_eq!(complete().capture().unwrap().source.project_fingerprint, 0);
}

#[test]
fn strict_consumption_requires_the_actual_full_strict_record_instance_without_fallback() {
    let mut builder = Builder::new();
    builder.add_entry(header(1), initial()).unwrap();
    builder
        .record_initial_capture(header(2), CaptureLabel::Initial, None, 1, 1, source())
        .unwrap();
    builder.add_entry(header(3), strict()).unwrap();
    builder.add_entry(header(4), undo(3)).unwrap();
    let before = builder.clone();
    assert_eq!(
        builder.record_strict_consumption(header(5), None, 1, 2, source()),
        Err(Error::MissingStrictInstance)
    );
    assert_eq!(
        builder.record_strict_consumption(header(5), Some(undo(3)), 1, 2, source()),
        Err(Error::WrongClonePhase)
    );
    let mut invented = strict();
    invented.worker_id = initial().worker_id;
    assert_eq!(
        builder.record_strict_consumption(header(5), Some(invented), 1, 2, source()),
        Err(Error::ChangedTuple)
    );
    assert_eq!(builder, before);
}

#[test]
fn all_seven_source_stamp_fields_must_match_capture_at_consumption() {
    for field in 0..7 {
        let mut builder = Builder::new();
        builder.add_entry(header(1), initial()).unwrap();
        builder
            .record_initial_capture(header(2), CaptureLabel::Initial, None, 1, 1, source())
            .unwrap();
        builder.add_entry(header(3), strict()).unwrap();
        let before = builder.clone();
        let mut changed = source();
        match field {
            0 => changed.project_session += 1,
            1 => changed.project_fingerprint += 1,
            2 => changed.timeline_revision += 1,
            3 => changed.epoch += 1,
            4 => changed.endpoint_id += 1,
            5 => changed.channel_id += 1,
            _ => changed.instance_id += 1,
        }
        assert_eq!(
            builder.record_strict_consumption(header(4), Some(strict()), 1, 2, changed),
            Err(Error::StampMismatch)
        );
        assert_eq!(builder, before);
    }
}

#[test]
fn exactly_one_capture_one_selected_clone_and_one_consumption_are_admitted() {
    let mut builder = complete_builder();
    let before = builder.clone();
    assert_eq!(
        builder.record_initial_capture(header(50), CaptureLabel::Initial, None, 1, 1, source()),
        Err(Error::DuplicateCapture)
    );
    assert_eq!(
        builder.record_strict_consumption(header(50), Some(strict()), 1, 20, source()),
        Err(Error::DuplicateConsumption)
    );
    let mut another = strict();
    another.ordinal = 9;
    assert_eq!(
        builder.add_entry(header(50), another),
        Err(Error::MultipleStrictClone)
    );
    assert_eq!(builder, before);
}

#[test]
fn consumption_without_capture_and_mismatched_capture_references_fail_without_mutation() {
    let mut builder = Builder::new();
    builder.add_entry(header(1), initial()).unwrap();
    builder.add_entry(header(2), strict()).unwrap();
    let before = builder.clone();
    assert_eq!(
        builder.record_strict_consumption(header(3), Some(strict()), 1, 1, source()),
        Err(Error::MissingCapture)
    );
    assert_eq!(builder, before);
    builder
        .record_initial_capture(header(3), CaptureLabel::Initial, None, 1, 1, source())
        .unwrap();
    let before = builder.clone();
    assert_eq!(
        builder.record_strict_consumption(header(4), Some(strict()), 1, 2, source()),
        Err(Error::CaptureReferenceMismatch)
    );
    assert_eq!(
        builder.record_strict_consumption(header(4), Some(strict()), 2, 3, source()),
        Err(Error::CaptureReferenceMismatch)
    );
    assert_eq!(builder, before);
}

#[test]
fn subset_sequence_gaps_and_section_order_preserve_exact_observed_headers() {
    let mut builder = Builder::new();
    builder.add_entry(header(1), initial()).unwrap();
    builder
        .record_initial_capture(header(50), CaptureLabel::Initial, None, 1, 100, source())
        .unwrap();
    builder.add_entry(header(10_004), strict()).unwrap();
    builder
        .record_strict_consumption(header(50_000), Some(strict()), 1, 50, source())
        .unwrap();
    let dictionary = builder.freeze().unwrap();
    let decoded = decode(encode(&dictionary).unwrap().as_bytes()).unwrap();
    assert_eq!(decoded, dictionary);
    assert_eq!(decoded.capture().unwrap().header, header(50));
    assert_eq!(decoded.entry(2).unwrap().construction, header(10_004));
}

#[test]
fn incomplete_dictionary_remains_explicitly_incomplete_and_empty_is_rejected() {
    assert_eq!(Builder::new().freeze(), Err(Error::Empty));
    let mut builder = Builder::new();
    builder.add_entry(header(1), initial()).unwrap();
    let entries_only = builder.clone().freeze().unwrap();
    assert!(
        !decode(encode(&entries_only).unwrap().as_bytes())
            .unwrap()
            .links_complete()
    );
    builder
        .record_initial_capture(header(2), CaptureLabel::Initial, None, 1, 9, source())
        .unwrap();
    let capture_only = builder.freeze().unwrap();
    let bytes = encode(&capture_only).unwrap();
    assert_eq!(&bytes.as_bytes()[20..24], &1u32.to_le_bytes());
    assert!(!decode(bytes.as_bytes()).unwrap().links_complete());
}

#[test]
fn decoder_rejects_unknown_magic_version_rows_flags_counts_and_reserved_fields() {
    let cases = [
        (8, 2),
        (12, 0),
        (12, 5),
        (16, 63),
        (20, 2),
        (20, 4),
        (32 + 8, 4),
        (32 + 12, 1),
        (160 + 4, 1),
        (256 + 4, 1),
        (160, 9),
        (256, 9),
    ];
    for (offset, value) in cases {
        let mut bytes = wire();
        put_u32(&mut bytes, offset, value);
        assert!(decode(&bytes).is_err(), "offset {offset}");
    }
    let mut bytes = wire();
    bytes[0] ^= 1;
    assert_eq!(decode(&bytes), Err(Error::BadMagic));
}

#[test]
fn decoder_requires_canonical_optional_presence_and_absent_values() {
    for offset in [32 + 16, 32 + 32] {
        let mut bytes = wire();
        put_u64(&mut bytes, offset, 2);
        assert_eq!(decode(&bytes), Err(Error::OptionalId));
    }
    for offset in [96 + 24, 96 + 40] {
        let mut bytes = wire();
        put_u64(&mut bytes, offset, 1);
        assert_eq!(decode(&bytes), Err(Error::OptionalId));
    }
}

#[test]
fn decoder_rejects_every_truncated_prefix_and_any_extra_suffix() {
    let bytes = wire();
    for end in 0..bytes.len() {
        assert!(decode(&bytes[..end]).is_err(), "prefix {end}");
    }
    let mut extra = bytes.clone();
    extra.push(0);
    assert_eq!(decode(&extra), Err(Error::ExtraBytes));
    let mut malformed = bytes;
    put_u64(&mut malformed, 24, 359);
    assert_eq!(decode(&malformed), Err(Error::BadHeader));
}

#[test]
fn decoder_checks_native_worker_and_receipt_usize_overflow_and_channel_u32_overflow() {
    let mut worker = wire();
    put_u64(&mut worker, 32 + 24, u64::from(u32::MAX) + 1);
    assert_eq!(
        decode_for_32_bit_fixture(&worker),
        Err(Error::WorkerOverflow)
    );
    let mut receipt = wire();
    put_u64(&mut receipt, 160 + 32, u64::from(u32::MAX) + 1);
    assert_eq!(
        decode_for_32_bit_fixture(&receipt),
        Err(Error::ReceiptOverflow)
    );
    #[cfg(target_pointer_width = "64")]
    {
        assert_eq!(
            decode(&worker)
                .unwrap()
                .entry(1)
                .unwrap()
                .identity
                .worker_id,
            Some(u32::MAX as usize + 1)
        );
        assert_eq!(
            decode(&receipt)
                .unwrap()
                .capture()
                .unwrap()
                .receipt_identity,
            u32::MAX as usize + 1
        );
    }
    let mut channel = wire();
    put_u64(&mut channel, 160 + 80, u64::from(u32::MAX) + 1);
    assert_eq!(decode(&channel), Err(Error::ChannelOverflow));
}

#[test]
fn decoder_binds_duplicate_ordinals_entry_order_global_sequences_and_timestamps() {
    let mut duplicated = wire();
    put_u64(&mut duplicated, 96, 1);
    assert_eq!(decode(&duplicated), Err(Error::DuplicateOrdinal));
    let mut reversed = wire();
    put_u64(&mut reversed, 96 + 48, 9);
    assert_eq!(decode(&reversed), Err(Error::SequenceOrder));
    let mut regressed = wire();
    put_u64(&mut regressed, 96 + 56, 99);
    assert_eq!(decode(&regressed), Err(Error::TimestampOrder));
    let mut zero = wire();
    put_u64(&mut zero, 32, 0);
    assert_eq!(decode(&zero), Err(Error::ZeroOrdinal));
    let mut zero_seq = wire();
    put_u64(&mut zero_seq, 160 + 8, 0);
    assert_eq!(decode(&zero_seq), Err(Error::ZeroSequence));
    let mut duplicate_event = wire();
    put_u64(&mut duplicate_event, 160 + 8, 30);
    put_u64(&mut duplicate_event, 256 + 40, 30);
    assert_eq!(decode(&duplicate_event), Err(Error::SequenceOrder));
}

#[test]
fn decoder_binds_capture_consumption_refs_and_all_seven_stamp_words() {
    for word in 0..7 {
        let mut bytes = wire();
        let offset = 256 + 48 + word * 8;
        let old = u64::from_le_bytes(bytes[offset..offset + 8].try_into().unwrap());
        put_u64(&mut bytes, offset, old + 1);
        assert_eq!(
            decode(&bytes),
            Err(Error::StampMismatch),
            "stamp word {word}"
        );
    }
    let mut reference = wire();
    put_u64(&mut reference, 256 + 40, 21);
    assert_eq!(decode(&reference), Err(Error::CaptureReferenceMismatch));
    let mut owner = wire();
    put_u64(&mut owner, 256 + 32, 2);
    assert_eq!(decode(&owner), Err(Error::CaptureReferenceMismatch));
    let mut missing_clone = wire();
    put_u64(&mut missing_clone, 256 + 24, 77);
    assert_eq!(decode(&missing_clone), Err(Error::UnknownReference));
}

#[test]
fn decoder_requires_capture_before_consumption_and_entry_construction_before_use() {
    let mut before_capture = wire();
    put_u64(&mut before_capture, 96 + 48, 15);
    put_u64(&mut before_capture, 96 + 56, 150);
    put_u64(&mut before_capture, 256 + 8, 19);
    put_u64(&mut before_capture, 256 + 16, 190);
    assert_eq!(decode(&before_capture), Err(Error::CaptureNotEarlier));
    let mut before_entry = wire();
    put_u64(&mut before_entry, 160 + 8, 9);
    assert_eq!(decode(&before_entry), Err(Error::EntryNotEarlier));
}

#[test]
fn actual_observer_identity_types_keep_every_tuple_field_and_explicit_wire_phase_codes() {
    // These operands are the actual parent types, not a separately defined
    // fixture representation. In particular their enum discriminants do not
    // determine the canonical wire codes 1/2/3.
    let actual: [super::InstanceIdentity; 3] = [
        super::InstanceIdentity {
            ordinal: 17,
            phase: super::InstancePhase::InitialSource,
            worker_id: None,
            source_instance_id: Some(0),
        },
        super::InstanceIdentity {
            ordinal: 19,
            phase: super::InstancePhase::StrictClone,
            worker_id: Some(0),
            source_instance_id: None,
        },
        super::InstanceIdentity {
            ordinal: 23,
            phase: super::InstancePhase::UndoSource,
            worker_id: Some(usize::MAX),
            source_instance_id: Some(u64::MAX),
        },
    ];
    let mut builder = Builder::new();
    for (index, identity) in actual.iter().enumerate() {
        builder
            .add_entry(header(index as u64 + 1), *identity)
            .unwrap();
    }
    let dictionary = builder.freeze().unwrap();
    let encoded = encode(&dictionary).unwrap();
    let decoded = decode(encoded.as_bytes()).unwrap();
    for (index, identity) in actual.iter().enumerate() {
        let actual_decoded: super::InstanceIdentity =
            decoded.entry(identity.ordinal).unwrap().identity;
        assert_eq!(actual_decoded, *identity);
        let phase_offset = 32 + index * 64 + 8;
        assert_eq!(
            u32::from_le_bytes(
                encoded.as_bytes()[phase_offset..phase_offset + 4]
                    .try_into()
                    .unwrap()
            ),
            index as u32 + 1
        );
    }
    let mut changed = actual[0];
    changed.worker_id = Some(0);
    assert_eq!(decoded.lookup_exact(changed), Err(Error::ChangedTuple));
    assert!(!decoded.sdk_authority());
}

#[test]
fn actual_bounce_source_type_preserves_all_seven_words_without_surrogate_conversion() {
    let actual: crate::instrument_bounce::BounceSourceIdentity =
        crate::instrument_bounce::BounceSourceIdentity {
            project_session: 0x0123_4567_89ab_cdef,
            project_fingerprint: u64::MAX,
            timeline_revision: 0xf123_4567_89ab_cdef,
            epoch: 0xe123_4567_89ab_cdef,
            endpoint_id: 0xd123_4567_89ab_cdef,
            channel_id: u32::MAX,
            instance_id: 0xc123_4567_89ab_cdef,
        };
    let actual_initial = super::InstanceIdentity {
        source_instance_id: Some(actual.instance_id),
        ..initial()
    };
    let actual_clone: super::InstanceIdentity = strict();
    let mut builder = Builder::new();
    builder.add_entry(header(10), actual_initial).unwrap();
    builder
        .record_initial_capture(
            header(20),
            super::CaptureLabel::Initial,
            None,
            actual_initial.ordinal,
            usize::MAX,
            actual,
        )
        .unwrap();
    builder.add_entry(header(30), actual_clone).unwrap();
    builder
        .record_strict_consumption(
            header(40),
            Some(actual_clone),
            actual_initial.ordinal,
            20,
            actual,
        )
        .unwrap();
    let dictionary = builder.freeze().unwrap();
    let decoded = decode(encode(&dictionary).unwrap().as_bytes()).unwrap();
    let actual_capture: crate::instrument_bounce::BounceSourceIdentity =
        decoded.capture().unwrap().source;
    let actual_consumed: crate::instrument_bounce::BounceSourceIdentity =
        decoded.consumption().unwrap().source;
    assert_eq!(actual_capture, actual);
    assert_eq!(actual_consumed, actual);
    assert_eq!(decoded.capture().unwrap().receipt_identity, usize::MAX);
    assert_eq!(
        decoded
            .entry(actual_clone.ordinal)
            .unwrap()
            .identity
            .worker_id,
        None
    );
    assert_eq!(
        decoded
            .entry(actual_clone.ordinal)
            .unwrap()
            .identity
            .source_instance_id,
        None
    );
    assert!(!decoded.sdk_authority());
}

#[test]
fn supplied_capture_link_leaves_the_actual_original_envelope_none_and_stage_unchanged() {
    let original = super::stream_envelope::Header {
        sequence: 20,
        monotonic_ns: 200,
        stage: super::Stage::WholeExport,
        instance: None,
        kind: 9,
        lane: super::stream_envelope::Lane::State,
    };
    // This body is a synthetic framing operand, not an opaque-state codec or
    // native capture receipt. Only original envelope immutability is tested.
    let mut envelope = Vec::with_capacity(super::stream_envelope::MAX_OPERAND);
    envelope.push(0x41);
    super::stream_envelope::wrap(original, &mut envelope).unwrap();
    let original_bytes = envelope.clone();
    let mut builder = Builder::new();
    builder.add_entry(header(10), initial()).unwrap();
    builder
        .record_initial_capture(
            RecordHeader {
                global_sequence: original.sequence,
                monotonic_elapsed_ns: original.monotonic_ns,
            },
            super::CaptureLabel::Initial,
            original.instance,
            initial().ordinal,
            31,
            source(),
        )
        .unwrap();
    let dictionary = builder.freeze().unwrap();
    let decoded_link = decode(encode(&dictionary).unwrap().as_bytes())
        .unwrap()
        .capture()
        .unwrap();
    assert_eq!(decoded_link.known_owner_source_ordinal, 1);
    assert_eq!(decoded_link.header.global_sequence, original.sequence);
    assert_eq!(
        decoded_link.header.monotonic_elapsed_ns,
        original.monotonic_ns
    );
    assert_eq!(envelope, original_bytes);
    let (actual_header, body) = super::stream_envelope::decode(&envelope).unwrap();
    assert_eq!(actual_header, original);
    assert_eq!(actual_header.instance, None);
    assert_eq!(actual_header.stage, super::Stage::WholeExport);
    assert_eq!(body, &[0x41]);
    assert!(!dictionary.sdk_authority());
}
