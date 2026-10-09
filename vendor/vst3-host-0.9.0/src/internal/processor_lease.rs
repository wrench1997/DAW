//! Narrow, borrowed access to the VST3 processing interface.
//!
//! The owning COM reference remains in `ControlDomain`. The exclusive borrow is the
//! synchronous quiescence fence: while the lease is live its owner cannot be torn down,
//! reconfigured or used for another processor call. No clone, cast, addRef/release or
//! component/controller access is available. Both lease and helper owner are !Send/!Sync;
//! this stage intentionally grants no permission to run a worker.
//!
//! Steinberg documents `setupProcessing`/bus metadata on the UI thread and only
//! `process`/`setProcessing` in the processing domain:
//! https://steinbergmedia.github.io/vst3_doc/vstinterfaces/classSteinberg_1_1Vst_1_1IAudioProcessor.html

use super::module_loader::VstModule;
use std::{marker::PhantomData, rc::Rc};
use vst3::{
    ComPtr,
    Steinberg::{
        tresult,
        Vst::{IAudioProcessor, IAudioProcessorTrait, ProcessData},
    },
};

/// Kept private to the crate, and intentionally narrower than IAudioProcessor itself.
pub(super) trait ProcessorCalls {
    /// Caller must provide the SDK's valid, exclusively owned processing buffers/queues.
    unsafe fn process(&mut self, data: &mut ProcessData) -> tresult;
    fn set_processing(&mut self, state: u8) -> tresult;
}

pub(super) struct ProcessorLease<'owner> {
    processor: &'owner mut ComPtr<IAudioProcessor>,
    _module: &'owner dyn VstModule,
    _same_thread: PhantomData<Rc<()>>,
}

impl<'owner> ProcessorLease<'owner> {
    pub(super) fn new(
        processor: &'owner mut ComPtr<IAudioProcessor>,
        module: &'owner dyn VstModule,
    ) -> Self {
        Self {
            processor,
            _module: module,
            _same_thread: PhantomData,
        }
    }
}

impl ProcessorCalls for ProcessorLease<'_> {
    unsafe fn process(&mut self, data: &mut ProcessData) -> tresult {
        // SAFETY: the caller provides live ProcessData buffers; the exclusive owner borrow
        // keeps its owning COM reference (and enclosing module) alive for this call.
        unsafe { self.processor.process(data) }
    }

    fn set_processing(&mut self, state: u8) -> tresult {
        // SAFETY: private lifecycle callers ensure the component is active and no process
        // call overlaps. The lease cannot be sent to another thread or outlive the owner.
        unsafe { self.processor.setProcessing(state) }
    }
}
