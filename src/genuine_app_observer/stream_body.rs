//! Complete canonical framing of borrowed control operands. No collector or
//! SDK is activated here; a live stream still owns sequencing and retirement.
use super::{
    stream_app_control as app, stream_codec as native, stream_control as control,
    stream_envelope as outer,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Error {
    HeaderMismatch,
    Envelope(outer::Error),
    Control(control::Error),
    App(app::AppError),
    Native(native::CodecError),
}

pub(super) fn encode_control(
    header: outer::Header,
    view: control::View<'_>,
    destination: &mut Vec<u8>,
) -> Result<(), Error> {
    if header.lane != outer::Lane::Control || header.kind != view.kind() {
        return Err(Error::HeaderMismatch);
    }
    control::encode_with_limit(view, destination, outer::MAX_BODY).map_err(Error::Control)?;
    if let Err(error) = outer::wrap(header, destination) {
        destination.clear();
        return Err(Error::Envelope(error));
    }
    Ok(())
}

pub(super) fn encode_app(
    header: outer::Header,
    view: app::AppView<'_>,
    destination: &mut Vec<u8>,
) -> Result<(), Error> {
    if header.lane != outer::Lane::App || header.kind != view.kind() {
        return Err(Error::HeaderMismatch);
    }
    app::encode_with_limit(view, destination, outer::MAX_BODY).map_err(Error::App)?;
    if let Err(error) = outer::wrap(header, destination) {
        destination.clear();
        return Err(Error::Envelope(error));
    }
    Ok(())
}

pub(super) fn decode_control(bytes: &[u8]) -> Result<(outer::Header, control::Control<'_>), Error> {
    let (header, body) = outer::decode(bytes).map_err(Error::Envelope)?;
    if header.lane != outer::Lane::Control {
        return Err(Error::HeaderMismatch);
    }
    let value = control::decode(body, header.kind).map_err(Error::Control)?;
    Ok((header, value))
}
pub(super) fn decode_app(bytes: &[u8]) -> Result<(outer::Header, app::AppControl<'_>), Error> {
    let (header, body) = outer::decode(bytes).map_err(Error::Envelope)?;
    if header.lane != outer::Lane::App {
        return Err(Error::HeaderMismatch);
    }
    let value = app::decode(body, header.kind).map_err(Error::App)?;
    Ok((header, value))
}

pub(super) fn encode_native(
    header: outer::Header,
    view: native::NativeView<'_>,
    dictionary: &native::BaselineDictionary,
    destination: &mut Vec<u8>,
) -> Result<(), Error> {
    if header.lane != outer::Lane::Native(view.tag()) || header.kind != view.kind() {
        return Err(Error::HeaderMismatch);
    }
    native::encode_native_body_with_limit(view, dictionary, destination, outer::MAX_BODY)
        .map_err(Error::Native)?;
    if let Err(error) = outer::wrap(header, destination) {
        destination.clear();
        return Err(Error::Envelope(error));
    }
    Ok(())
}

pub(super) fn decode_native<'a>(
    bytes: &'a [u8],
    dictionary: &'a native::BaselineDictionary,
) -> Result<(outer::Header, native::NativeBody<'a>), Error> {
    let (header, body) = outer::decode(bytes).map_err(Error::Envelope)?;
    let outer::Lane::Native(tag) = header.lane else {
        return Err(Error::HeaderMismatch);
    };
    let value = native::decode_native_body_with_limit(tag, body, dictionary, outer::MAX_BODY)
        .map_err(Error::Native)?;
    if value.kind() != header.kind {
        return Err(Error::HeaderMismatch);
    }
    Ok((header, value))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::genuine_app_observer::{InstanceIdentity, InstancePhase, Stage};
    fn header(kind: u16, lane: outer::Lane) -> outer::Header {
        outer::Header {
            sequence: 17,
            monotonic_ns: 29,
            stage: Stage::WholeExport,
            instance: Some(InstanceIdentity {
                ordinal: 2,
                phase: InstancePhase::StrictClone,
                worker_id: None,
                source_instance_id: Some(0),
            }),
            kind,
            lane,
        }
    }
    fn dictionary() -> native::BaselineDictionary {
        native::BaselineDictionary::new(
            1,
            native::CanonicalMetadata {
                class_uid: "pure test fixture".into(),
                version: "1".into(),
                name: "fixture".into(),
                category: "Instrument".into(),
                audio: native::AudioLayout::default(),
                events: native::EventLayout::default(),
                latency_samples: 0,
                tail_samples: 0,
            },
        )
        .unwrap()
    }

    #[test]
    fn complete_native_midi_envelope_preserves_header_and_actual_data() {
        let header = header(8, outer::Lane::Native(6));
        let dictionary = dictionary();
        let mut encoded = Vec::with_capacity(outer::MAX_OPERAND);
        let pointer = encoded.as_ptr();
        encode_native(
            header,
            native::NativeView::Midi {
                data: [0x90, 60, 100],
                offset: 127,
            },
            &dictionary,
            &mut encoded,
        )
        .unwrap();
        assert_eq!(encoded.as_ptr(), pointer);
        let (actual, value) = decode_native(&encoded, &dictionary).unwrap();
        assert_eq!(actual, header);
        let native::NativeBody::Midi { data, offset } = value else {
            panic!("wrong native body");
        };
        assert_eq!((data, offset), ([0x90, 60, 100], 127));
        assert!(decode_control(&encoded).is_err());
    }

    #[test]
    fn native_strict_failure_never_becomes_a_shortened_success_or_string() {
        use crate::genuine_app_observer::CallOutcome;
        use crate::instrument_bounce::render::{BackendFailure, BackendFailureKind};
        let failure = CallOutcome::StrictError(BackendFailure::new(
            BackendFailureKind::Backend,
            "actual strict error",
        ));
        let mut encoded = Vec::with_capacity(outer::MAX_OPERAND);
        let result = encode_native(
            header(1, outer::Lane::Native(11)),
            native::NativeView::Call {
                operation: native::NativeOperation::Process,
                boundary: native::Boundary::Returned,
                frames: None,
                transport: None,
                outcome: Some(&failure),
            },
            &dictionary(),
            &mut encoded,
        );
        assert_eq!(
            result,
            Err(Error::Native(native::CodecError::TypedControlRequired))
        );
        assert!(encoded.is_empty());
    }
    #[test]
    fn complete_control_envelope_keeps_real_header_and_length_only_state() {
        let header = header(16, outer::Lane::Control);
        let mut bytes = Vec::with_capacity(outer::MAX_OPERAND);
        let allocation = bytes.as_ptr();
        encode_control(
            header,
            control::View::OrdinaryState {
                endpoint_id: 41,
                project_session: 43,
                instance_id: 47,
                slot: 3,
                request_id: 53,
                epoch: Some(0),
                byte_len: 591_772,
            },
            &mut bytes,
        )
        .unwrap();
        assert_eq!(bytes.as_ptr(), allocation);
        let (actual_header, body) = decode_control(&bytes).unwrap();
        assert_eq!(actual_header, header);
        let control::Control::OrdinaryState {
            endpoint_id,
            project_session,
            instance_id,
            slot,
            request_id,
            epoch,
            byte_len,
        } = body
        else {
            panic!("wrong control");
        };
        assert_eq!(
            (
                endpoint_id,
                project_session,
                instance_id,
                slot,
                request_id,
                epoch,
                byte_len
            ),
            (41, 43, 47, 3, 53, Some(0), 591_772)
        );
        assert!(bytes.len() < 160);
    }
    #[test]
    fn wrong_control_header_is_not_silently_relabelled_or_written() {
        let mut bytes = Vec::with_capacity(outer::MAX_OPERAND);
        encode_control(
            header(23, outer::Lane::Control),
            control::View::Lifecycle("real site"),
            &mut bytes,
        )
        .unwrap();
        let original = bytes.clone();
        assert_eq!(
            encode_control(
                header(22, outer::Lane::Control),
                control::View::Lifecycle("other"),
                &mut bytes
            ),
            Err(Error::HeaderMismatch)
        );
        assert_eq!(bytes, original);
        assert!(matches!(decode_app(&bytes), Err(Error::HeaderMismatch)));
    }
    #[test]
    fn complete_app_envelope_validates_body_after_header_parse() {
        use crate::audio::{AudioAssetEvent, AudioAssetOperation};
        let event = AudioAssetEvent::Unregistered {
            operation: AudioAssetOperation {
                engine_session: 61,
                generation: 67,
                operation_id: 71,
            },
            id: 73,
            removed: false,
        };
        let header = header(10, outer::Lane::App);
        let mut bytes = Vec::with_capacity(outer::MAX_OPERAND);
        encode_app(header, app::AppView::AudioAsset(&event), &mut bytes).unwrap();
        let (decoded, body) = decode_app(&bytes).unwrap();
        assert_eq!(decoded, header);
        let app::AppControl::AudioAsset(actual) = body else {
            panic!("wrong App control");
        };
        assert_eq!(actual, event);
        let (_, body) = outer::decode(&bytes).unwrap();
        let body_offset = bytes.len() - body.len();
        bytes[body_offset] = b'X';
        assert!(matches!(decode_app(&bytes), Err(Error::App(_))));
    }
}
