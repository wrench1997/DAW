//! A real no-editor VST3 Effect with a finite two-tap impulse response.
//!
//! For each independent channel, y[n] = gain * (x[n-32] + x[n-160] * 0.5).
//! Initial history is zero. The first response is delayed 32 samples; the last
//! tap is at 160 samples (128 samples after the latency-aligned direct tap).
//! There is no oscillator, MIDI input, audio device or native window.

#![allow(non_snake_case)]
// Generated SDK enum constants differ in signedness across supported targets.
#![allow(clippy::unnecessary_cast)]

use std::ffi::{c_char, c_void};
use std::ptr;
use std::slice;
use std::sync::Mutex;
use vst3::{uid, Class, ComRef, ComWrapper, Steinberg::Vst::*, Steinberg::*};

pub const CLASS_ID: &str = "43545253454658440000000000000001";
pub const NAME: &str = "Citrus Stereo Delay Fixture";
pub const VENDOR: &str = "Citrus Studio Test Fixtures";
pub const VERSION: &str = "1.0.0";
pub const CATEGORY: &str = "Fx|Delay";
pub const LATENCY: usize = 32;
pub const LAST_TAP: usize = 160;
const RING: usize = LAST_TAP + 1;
const CID: TUID = uid(0x43545253, 0x45465844, 0x00000000, 0x00000001);
const MAGIC: &[u8; 4] = b"CE01";
const PROBE_SCALE: f64 = 65_536.0;
const SAMPLE_POSITION_SCALE: f64 = 16_777_216.0;
const PROBES: &[(u32, &str)] = &[
    (1000, "Process Calls"),
    (1001, "Last Sample Position"),
    (1002, "Last Quarter Position"),
    (1003, "Last Tempo"),
    (1004, "Last Playing"),
    (1005, "Active Stereo Offline"),
    (1006, "Outputs Initially Clear"),
];

struct Engine {
    gain: f64,
    history: [[f32; RING]; 2],
    cursor: usize,
    sample_rate: f64,
    mode: i32,
    max_frames: i32,
    input_active: bool,
    output_active: bool,
    active: bool,
    processing: bool,
    process_calls: u32,
    last_sample: i64,
    last_quarter: f64,
    last_tempo: f64,
    last_playing: bool,
    outputs_initially_clear: bool,
}

impl Default for Engine {
    fn default() -> Self {
        Self {
            gain: 0.5,
            history: [[0.0; RING]; 2],
            cursor: 0,
            sample_rate: 48_000.0,
            mode: ProcessModes_::kRealtime as i32,
            max_frames: 0,
            input_active: false,
            output_active: false,
            active: false,
            processing: false,
            process_calls: 0,
            last_sample: 0,
            last_quarter: 0.0,
            last_tempo: 0.0,
            last_playing: false,
            outputs_initially_clear: false,
        }
    }
}

impl Engine {
    fn reset_history(&mut self) {
        self.history = [[0.0; RING]; 2];
        self.cursor = 0;
    }

    fn tick(&mut self, input: [f32; 2]) -> [f32; 2] {
        let direct = (self.cursor + RING - LATENCY) % RING;
        let echo = (self.cursor + RING - LAST_TAP) % RING;
        let mut output = [0.0; 2];
        for channel in 0..2 {
            self.history[channel][self.cursor] = input[channel];
            output[channel] = self.gain as f32
                * (self.history[channel][direct] + 0.5 * self.history[channel][echo]);
        }
        self.cursor = (self.cursor + 1) % RING;
        output
    }

    fn probe(&self, id: u32) -> f64 {
        match id {
            0 => self.gain,
            1000 => f64::from(self.process_calls) / PROBE_SCALE,
            1001 => self.last_sample as f64 / SAMPLE_POSITION_SCALE,
            1002 => self.last_quarter / PROBE_SCALE,
            1003 => self.last_tempo / 1024.0,
            1004 => f64::from(u8::from(self.last_playing)),
            1005 => f64::from(u8::from(
                self.active
                    && self.processing
                    && self.input_active
                    && self.output_active
                    && self.mode == ProcessModes_::kOffline as i32,
            )),
            1006 => f64::from(u8::from(self.outputs_initially_clear)),
            _ => 0.0,
        }
    }
}

#[derive(Default)]
struct Effect {
    engine: Mutex<Engine>,
}

impl Class for Effect {
    type Interfaces = (
        IComponent,
        IAudioProcessor,
        IEditController,
        IProcessContextRequirements,
    );
}

fn copy_cstr(value: &str, target: &mut [c_char]) {
    target.fill(0);
    let count = value.len().min(target.len().saturating_sub(1));
    for (slot, byte) in target.iter_mut().zip(value.bytes()).take(count) {
        *slot = byte as c_char;
    }
}

fn copy_utf16(value: &str, target: &mut [TChar]) {
    target.fill(0);
    let count = target.len().saturating_sub(1);
    for (slot, unit) in target.iter_mut().zip(value.encode_utf16()).take(count) {
        *slot = unit;
    }
}

pub fn state_bytes(gain: f64) -> Option<[u8; 12]> {
    if !gain.is_finite() || !(0.0..=1.0).contains(&gain) {
        return None;
    }
    let mut bytes = [0u8; 12];
    bytes[..4].copy_from_slice(MAGIC);
    bytes[4..].copy_from_slice(&gain.to_le_bytes());
    Some(bytes)
}

fn decode_state(bytes: &[u8; 12]) -> Option<f64> {
    if &bytes[..4] != MAGIC {
        return None;
    }
    let gain = f64::from_le_bytes(bytes[4..].try_into().ok()?);
    state_bytes(gain).map(|_| gain)
}

unsafe fn read_state(stream: *mut IBStream) -> Option<f64> {
    let stream = ComRef::from_raw(stream)?;
    let mut bytes = [0u8; 12];
    let mut count = 0;
    if stream.read(bytes.as_mut_ptr().cast(), bytes.len() as i32, &mut count) != kResultOk
        || count != bytes.len() as i32
    {
        return None;
    }
    decode_state(&bytes)
}

unsafe fn apply_static_changes(engine: &mut Engine, changes: *mut IParameterChanges) -> tresult {
    if let Some(changes) = ComRef::from_raw(changes) {
        for queue_index in 0..changes.getParameterCount() {
            let Some(queue) = ComRef::from_raw(changes.getParameterData(queue_index)) else {
                return kResultFalse;
            };
            if queue.getParameterId() != 0 {
                return kResultFalse;
            }
            for point in 0..queue.getPointCount() {
                let mut offset = 0;
                let mut gain = 0.0;
                if queue.getPoint(point, &mut offset, &mut gain) != kResultOk
                    || offset != 0
                    || state_bytes(gain).is_none()
                {
                    return kResultFalse;
                }
                engine.gain = gain;
            }
        }
    }
    kResultOk
}

impl IPluginBaseTrait for Effect {
    unsafe fn initialize(&self, _context: *mut FUnknown) -> tresult {
        kResultOk
    }
    unsafe fn terminate(&self) -> tresult {
        kResultOk
    }
}

impl IComponentTrait for Effect {
    unsafe fn getControllerClassId(&self, _class_id: *mut TUID) -> tresult {
        // The controller is this same COM object, not another factory class.
        kNotImplemented
    }
    unsafe fn setIoMode(&self, _mode: IoMode) -> tresult {
        kResultOk
    }
    unsafe fn getBusCount(&self, media_type: MediaType, dir: BusDirection) -> i32 {
        i32::from(
            media_type as MediaTypes == MediaTypes_::kAudio
                && matches!(
                    dir as BusDirections,
                    BusDirections_::kInput | BusDirections_::kOutput
                ),
        )
    }
    unsafe fn getBusInfo(
        &self,
        media_type: MediaType,
        dir: BusDirection,
        index: i32,
        info: *mut BusInfo,
    ) -> tresult {
        if info.is_null() || index != 0 || self.getBusCount(media_type, dir) != 1 {
            return kInvalidArgument;
        }
        *info = std::mem::zeroed();
        (*info).mediaType = media_type;
        (*info).direction = dir;
        (*info).channelCount = 2;
        (*info).busType = BusTypes_::kMain as BusType;
        (*info).flags = BusInfo_::BusFlags_::kDefaultActive as u32;
        copy_utf16(
            if dir as BusDirections == BusDirections_::kInput {
                "Stereo In"
            } else {
                "Stereo Out"
            },
            &mut (*info).name,
        );
        kResultOk
    }
    unsafe fn getRoutingInfo(
        &self,
        _input: *mut RoutingInfo,
        _output: *mut RoutingInfo,
    ) -> tresult {
        kNotImplemented
    }
    unsafe fn activateBus(
        &self,
        media: MediaType,
        dir: BusDirection,
        index: i32,
        active: TBool,
    ) -> tresult {
        if index != 0 || self.getBusCount(media, dir) != 1 {
            return kInvalidArgument;
        }
        let Ok(mut engine) = self.engine.lock() else {
            return kResultFalse;
        };
        if dir as BusDirections == BusDirections_::kInput {
            engine.input_active = active != 0;
        } else {
            engine.output_active = active != 0;
        }
        kResultOk
    }
    unsafe fn setActive(&self, active: TBool) -> tresult {
        let Ok(mut engine) = self.engine.lock() else {
            return kResultFalse;
        };
        engine.active = active != 0;
        if active == 0 {
            engine.processing = false;
            engine.reset_history();
        }
        kResultOk
    }
    unsafe fn setState(&self, stream: *mut IBStream) -> tresult {
        let Some(gain) = read_state(stream) else {
            return kResultFalse;
        };
        let Ok(mut engine) = self.engine.lock() else {
            return kResultFalse;
        };
        engine.gain = gain;
        engine.reset_history();
        kResultOk
    }
    unsafe fn getState(&self, stream: *mut IBStream) -> tresult {
        let Some(stream) = ComRef::from_raw(stream) else {
            return kInvalidArgument;
        };
        let Ok(engine) = self.engine.lock() else {
            return kResultFalse;
        };
        let bytes = state_bytes(engine.gain).expect("validated gain");
        let mut count = 0;
        if stream.write(
            bytes.as_ptr().cast_mut().cast(),
            bytes.len() as i32,
            &mut count,
        ) == kResultOk
            && count == bytes.len() as i32
        {
            kResultOk
        } else {
            kResultFalse
        }
    }
}

impl IAudioProcessorTrait for Effect {
    unsafe fn setBusArrangements(
        &self,
        inputs: *mut SpeakerArrangement,
        num_inputs: i32,
        outputs: *mut SpeakerArrangement,
        num_outputs: i32,
    ) -> tresult {
        if num_inputs != 1
            || num_outputs != 1
            || inputs.is_null()
            || outputs.is_null()
            || *inputs != SpeakerArr::kStereo
            || *outputs != SpeakerArr::kStereo
        {
            return kResultFalse;
        }
        kResultOk
    }
    unsafe fn getBusArrangement(
        &self,
        dir: BusDirection,
        index: i32,
        arrangement: *mut SpeakerArrangement,
    ) -> tresult {
        if arrangement.is_null()
            || index != 0
            || !matches!(
                dir as BusDirections,
                BusDirections_::kInput | BusDirections_::kOutput
            )
        {
            return kInvalidArgument;
        }
        *arrangement = SpeakerArr::kStereo;
        kResultOk
    }
    unsafe fn canProcessSampleSize(&self, size: i32) -> tresult {
        if size == SymbolicSampleSizes_::kSample32 as i32 {
            kResultOk
        } else {
            kResultFalse
        }
    }
    unsafe fn getLatencySamples(&self) -> u32 {
        LATENCY as u32
    }
    unsafe fn getTailSamples(&self) -> u32 {
        LAST_TAP as u32
    }
    unsafe fn setupProcessing(&self, setup: *mut ProcessSetup) -> tresult {
        let Some(setup) = setup.as_ref() else {
            return kInvalidArgument;
        };
        if setup.symbolicSampleSize != SymbolicSampleSizes_::kSample32 as i32
            || !setup.sampleRate.is_finite()
            || setup.sampleRate <= 0.0
            || !(1..=2048).contains(&setup.maxSamplesPerBlock)
        {
            return kInvalidArgument;
        }
        let Ok(mut engine) = self.engine.lock() else {
            return kResultFalse;
        };
        engine.sample_rate = setup.sampleRate;
        engine.mode = setup.processMode;
        engine.max_frames = setup.maxSamplesPerBlock;
        engine.reset_history();
        kResultOk
    }
    unsafe fn setProcessing(&self, processing: TBool) -> tresult {
        let Ok(mut engine) = self.engine.lock() else {
            return kResultFalse;
        };
        if processing != 0 && !engine.active {
            return kResultFalse;
        }
        engine.processing = processing != 0;
        kResultOk
    }
    unsafe fn process(&self, data: *mut ProcessData) -> tresult {
        let Some(data) = data.as_mut() else {
            return kInvalidArgument;
        };
        let Ok(mut engine) = self.engine.lock() else {
            return kResultFalse;
        };
        // VST3 permits a stopped, zero-sample parameter flush with no audio
        // buffers. The normal host uses this before capturing opaque state.
        if data.numSamples == 0 {
            return apply_static_changes(&mut engine, data.inputParameterChanges);
        }
        if !engine.active
            || !engine.processing
            || !engine.input_active
            || !engine.output_active
            || data.processMode != ProcessModes_::kOffline as i32
            || engine.mode != data.processMode
            || data.symbolicSampleSize != SymbolicSampleSizes_::kSample32 as i32
            || !(1..=engine.max_frames).contains(&data.numSamples)
            || data.numInputs != 1
            || data.numOutputs != 1
            || data.inputs.is_null()
            || data.outputs.is_null()
        {
            return kResultFalse;
        }
        if let Some(events) = ComRef::from_raw(data.inputEvents) {
            if events.getEventCount() != 0 {
                return kResultFalse;
            }
        }
        let Some(context) = data.processContext.as_ref() else {
            return kResultFalse;
        };
        let required = (ProcessContext_::StatesAndFlags_::kProjectTimeMusicValid
            | ProcessContext_::StatesAndFlags_::kTempoValid
            | ProcessContext_::StatesAndFlags_::kTimeSigValid) as u32;
        if context.state & required != required
            || !context.tempo.is_finite()
            || context.tempo <= 0.0
            || !context.projectTimeMusic.is_finite()
            || context.sampleRate != engine.sample_rate
            || context.timeSigNumerator <= 0
            || !matches!(context.timeSigDenominator, 1 | 2 | 4 | 8 | 16)
        {
            return kResultFalse;
        }
        let input_bus = &*data.inputs;
        let output_bus = &mut *data.outputs;
        if input_bus.numChannels != 2
            || output_bus.numChannels != 2
            || input_bus.__field0.channelBuffers32.is_null()
            || output_bus.__field0.channelBuffers32.is_null()
        {
            return kResultFalse;
        }
        let input_ptrs = slice::from_raw_parts(input_bus.__field0.channelBuffers32, 2);
        let output_ptrs = slice::from_raw_parts(output_bus.__field0.channelBuffers32, 2);
        if input_ptrs
            .iter()
            .chain(output_ptrs)
            .any(|pointer| pointer.is_null())
        {
            return kResultFalse;
        }
        let frames = data.numSamples as usize;
        // Read input samples individually before the corresponding output write;
        // this also supports SDK-legal in-place processing without aliased slices.
        for pointer in input_ptrs {
            if (0..frames).any(|frame| !(*pointer.add(frame)).is_finite()) {
                return kResultFalse;
            }
        }
        engine.outputs_initially_clear = output_ptrs
            .iter()
            .all(|pointer| (0..frames).all(|frame| *pointer.add(frame) == 0.0));
        // The host may queue the same captured gain after state restoration.
        // This fixture has a single static writable parameter; no automation is
        // hidden in a whole-block last-point application.
        if apply_static_changes(&mut engine, data.inputParameterChanges) != kResultOk {
            return kResultFalse;
        }
        for frame in 0..frames {
            let output = engine.tick([*input_ptrs[0].add(frame), *input_ptrs[1].add(frame)]);
            *output_ptrs[0].add(frame) = output[0];
            *output_ptrs[1].add(frame) = output[1];
        }
        output_bus.silenceFlags =
            output_ptrs
                .iter()
                .enumerate()
                .fold(0, |flags, (channel, pointer)| {
                    flags
                        | if (0..frames).all(|frame| *pointer.add(frame) == 0.0) {
                            1 << channel
                        } else {
                            0
                        }
                });
        engine.process_calls += 1;
        engine.last_sample = context.projectTimeSamples;
        engine.last_quarter = context.projectTimeMusic;
        engine.last_tempo = context.tempo;
        engine.last_playing =
            context.state & ProcessContext_::StatesAndFlags_::kPlaying as u32 != 0;
        kResultOk
    }
}

impl IProcessContextRequirementsTrait for Effect {
    unsafe fn getProcessContextRequirements(&self) -> u32 {
        (IProcessContextRequirements_::Flags_::kNeedProjectTimeMusic
            | IProcessContextRequirements_::Flags_::kNeedTempo
            | IProcessContextRequirements_::Flags_::kNeedTimeSignature
            | IProcessContextRequirements_::Flags_::kNeedTransportState) as u32
    }
}

impl IEditControllerTrait for Effect {
    unsafe fn setComponentState(&self, stream: *mut IBStream) -> tresult {
        IComponentTrait::setState(self, stream)
    }
    unsafe fn setState(&self, _stream: *mut IBStream) -> tresult {
        kNotImplemented
    }
    unsafe fn getState(&self, _stream: *mut IBStream) -> tresult {
        kNotImplemented
    }
    unsafe fn getParameterCount(&self) -> i32 {
        1 + PROBES.len() as i32
    }
    unsafe fn getParameterInfo(&self, index: i32, info: *mut ParameterInfo) -> tresult {
        if info.is_null() || index < 0 {
            return kInvalidArgument;
        }
        let (id, name) = if index == 0 {
            (0, "Gain")
        } else if let Some(probe) = PROBES.get(index as usize - 1) {
            *probe
        } else {
            return kInvalidArgument;
        };
        *info = std::mem::zeroed();
        (*info).id = id;
        (*info).defaultNormalizedValue = if id == 0 { 0.5 } else { 0.0 };
        (*info).flags = if id == 0 {
            0
        } else {
            ParameterInfo_::ParameterFlags_::kIsReadOnly as i32
        };
        copy_utf16(name, &mut (*info).title);
        copy_utf16(name, &mut (*info).shortTitle);
        kResultOk
    }
    unsafe fn getParamStringByValue(
        &self,
        _id: u32,
        value: f64,
        target: *mut String128,
    ) -> tresult {
        if target.is_null() || !value.is_finite() {
            return kInvalidArgument;
        }
        copy_utf16(&format!("{value:.6}"), &mut *target);
        kResultOk
    }
    unsafe fn getParamValueByString(
        &self,
        _id: u32,
        _text: *mut TChar,
        _value: *mut f64,
    ) -> tresult {
        kNotImplemented
    }
    unsafe fn normalizedParamToPlain(&self, _id: u32, value: f64) -> f64 {
        value
    }
    unsafe fn plainParamToNormalized(&self, _id: u32, value: f64) -> f64 {
        value
    }
    unsafe fn getParamNormalized(&self, id: u32) -> f64 {
        self.engine
            .lock()
            .map(|engine| engine.probe(id))
            .unwrap_or(0.0)
    }
    unsafe fn setParamNormalized(&self, id: u32, gain: f64) -> tresult {
        if id != 0 || state_bytes(gain).is_none() {
            return kInvalidArgument;
        }
        let Ok(mut engine) = self.engine.lock() else {
            return kResultFalse;
        };
        engine.gain = gain;
        kResultOk
    }
    unsafe fn setComponentHandler(&self, _handler: *mut IComponentHandler) -> tresult {
        kResultOk
    }
    unsafe fn createView(&self, _name: *const c_char) -> *mut IPlugView {
        ptr::null_mut()
    }
}

struct Factory;
impl Class for Factory {
    type Interfaces = (IPluginFactory2,);
}

impl IPluginFactoryTrait for Factory {
    unsafe fn getFactoryInfo(&self, info: *mut PFactoryInfo) -> tresult {
        let Some(info) = info.as_mut() else {
            return kInvalidArgument;
        };
        *info = std::mem::zeroed();
        copy_cstr(VENDOR, &mut info.vendor);
        copy_cstr("https://github.com/wrench1997/DAW", &mut info.url);
        kResultOk
    }
    unsafe fn countClasses(&self) -> i32 {
        1
    }
    unsafe fn getClassInfo(&self, index: i32, info: *mut PClassInfo) -> tresult {
        if index != 0 || info.is_null() {
            return kInvalidArgument;
        }
        *info = std::mem::zeroed();
        (*info).cid = CID;
        (*info).cardinality = PClassInfo_::ClassCardinality_::kManyInstances as i32;
        copy_cstr("Audio Module Class", &mut (*info).category);
        copy_cstr(NAME, &mut (*info).name);
        kResultOk
    }
    unsafe fn createInstance(
        &self,
        cid: FIDString,
        iid: FIDString,
        object: *mut *mut c_void,
    ) -> tresult {
        if cid.is_null() || iid.is_null() || object.is_null() {
            return kInvalidArgument;
        }
        *object = ptr::null_mut();
        if *cid.cast::<TUID>() != CID {
            return kInvalidArgument;
        }
        let instance = ComWrapper::new(Effect::default())
            .to_com_ptr::<FUnknown>()
            .expect("FUnknown");
        let pointer = instance.as_ptr();
        ((*(*pointer).vtbl).queryInterface)(pointer, iid.cast_mut().cast(), object)
    }
}

impl IPluginFactory2Trait for Factory {
    unsafe fn getClassInfo2(&self, index: i32, info: *mut PClassInfo2) -> tresult {
        if index != 0 || info.is_null() {
            return kInvalidArgument;
        }
        *info = std::mem::zeroed();
        (*info).cid = CID;
        (*info).cardinality = PClassInfo_::ClassCardinality_::kManyInstances as i32;
        copy_cstr("Audio Module Class", &mut (*info).category);
        copy_cstr(NAME, &mut (*info).name);
        copy_cstr(CATEGORY, &mut (*info).subCategories);
        copy_cstr(VENDOR, &mut (*info).vendor);
        copy_cstr(VERSION, &mut (*info).version);
        copy_cstr("VST 3.8.0", &mut (*info).sdkVersion);
        kResultOk
    }
}

#[no_mangle]
extern "system" fn GetPluginFactory() -> *mut IPluginFactory {
    ComWrapper::new(Factory)
        .to_com_ptr::<IPluginFactory>()
        .expect("factory")
        .into_raw()
}

#[cfg(target_os = "windows")]
#[no_mangle]
extern "system" fn InitDll() -> bool {
    true
}
#[cfg(target_os = "windows")]
#[no_mangle]
extern "system" fn ExitDll() -> bool {
    true
}
#[cfg(target_os = "linux")]
#[no_mangle]
extern "system" fn ModuleEntry(_handle: *mut c_void) -> bool {
    true
}
#[cfg(target_os = "linux")]
#[no_mangle]
extern "system" fn ModuleExit() -> bool {
    true
}

#[cfg(test)]
mod tests;
