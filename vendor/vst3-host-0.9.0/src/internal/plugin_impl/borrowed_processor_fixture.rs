//! TEST-ONLY processor capability/lifetime proof. The entire module is cfg(test).
//!
//! This is not an alternate production Process path. It deliberately does not run the
//! runtime's native-input acknowledgment, positive-call history, output-fault checks,
//! transport, event or parameter orchestration. Those must remain intact in a separately
//! reviewed production integration. Existing ProcessorLease/ProcessorOps stay !Send.
//!
//! One token exclusively borrows the aggregate owner, which physically owns every COM
//! reference AND its module. The private pointer never owns or releases a COM reference.
//! std::thread::scope is the test's quiescence boundary: it joins before the owner borrow
//! ends, including Rust panic. A blocked worker blocks that boundary; it cannot be detached
//! to permit unloading. No claim is made about unwinding through a foreign COM function.

use super::{PluginImpl, Result};
use crate::error::Error;
use crate::internal::module_loader::VstModule;
use std::{
    cell::Cell,
    marker::PhantomData,
    ptr::NonNull,
    sync::atomic::{AtomicBool, Ordering},
};
use vst3::{
    ComRef,
    Steinberg::{
        kInternalError, tresult,
        Vst::{IAudioProcessor, IAudioProcessorTrait, ProcessData},
    },
};

/// Not Clone and explicitly !Sync. An exclusive mutable receiver serializes SDK calls.
/// No COM extractor, QI/cast/refcount, metadata, controller or lifecycle capability exists.
pub(crate) struct BorrowedProcessor<'owner> {
    processor: NonNull<IAudioProcessor>,
    in_process: &'owner AtomicBool,
    _owner_and_module: PhantomData<(&'owner mut PluginImpl, &'owner dyn VstModule)>,
    _not_sync: PhantomData<Cell<()>>,
}

// SAFETY: Steinberg's IAudioProcessor::process is explicitly a processing-thread method:
// https://steinbergmedia.github.io/vst3_doc/vstinterfaces/classSteinberg_1_1Vst_1_1IAudioProcessor.html
// This sole test-only Send implementation transfers a non-owning call capability, NOT a
// ComPtr, owner, module, or runtime. Its private constructor exclusively borrows the fully
// active/prepared owner for 'owner. That owner retains the processor and module on the
// loading thread; it cannot be accessed or destroyed through safe Rust until the token
// loan ends. The token cannot escape into an unscoped 'static thread. Moving/dropping it
// performs no AddRef/Release and never dereferences the phantom owner/module. Only process
// dereferences the interface; its unsafe caller must supply exclusive, live SDK data.
// &mut self and !Sync prevent safe concurrent calls through the same token. The separate
// process flag is an AtomicBool borrowed from the pinned-in-scope host context, not host
// receiver/run-loop/teardown authority. Tests use std::thread::scope, which cannot detach.
unsafe impl Send for BorrowedProcessor<'_> {}

/// Loading-thread preflight only. No processor, metadata or owning-reference call occurs.
pub(crate) fn borrow_for_test(owner: &mut PluginImpl) -> Result<BorrowedProcessor<'_>> {
    owner.ensure_control_thread("test-only processor borrow")?;
    if !owner.control.is_active
        || !owner.runtime.is_processing
        || owner.control.applied_setup.is_none()
        || owner.runtime.process_data.is_none()
        || owner.runtime.observe_parameter_output_fault()
    {
        return Err(Error::Other(
            "test-only processor owner is not ready".into(),
        ));
    }
    let in_process = owner
        .control
        ._host_app
        .data_exchange_process_gate()
        .flag_for_scoped_worker_test();
    if in_process.load(Ordering::Acquire) {
        return Err(Error::Other(
            "test-only processor call already active".into(),
        ));
    }
    // ComPtr is non-null by construction and remains owned, without cloning, by owner.
    let processor = NonNull::new(owner.control.processor.as_ptr())
        .expect("owning processor interface is non-null");
    Ok(BorrowedProcessor {
        processor,
        in_process,
        _owner_and_module: PhantomData,
        _not_sync: PhantomData,
    })
}

impl BorrowedProcessor<'_> {
    /// # Safety
    /// The caller must provide SDK-valid ProcessData, including every nested pointer,
    /// buffer/COM queue lifetime and exclusive writable region, for the whole call. The
    /// data must match the owner's prepared format/buses/maximum and processing state.
    /// A mutable ProcessData reference alone does NOT establish these requirements.
    pub(crate) unsafe fn process(&mut self, data: &mut ProcessData) -> tresult {
        if self
            .in_process
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return kInternalError;
        }
        let _active = ProcessFlagGuard(self.in_process);
        // SAFETY: the constructor's exclusive owner/module loan keeps this exact interface
        // alive. The unsafe caller guarantees all ProcessData pointees and SDK preconditions.
        // ComRef is borrowed; constructing/dropping it performs no AddRef or Release.
        unsafe {
            ComRef::from_raw(self.processor.as_ptr())
                .expect("non-null borrowed processor")
                .process(data)
        }
    }
}

struct ProcessFlagGuard<'call>(&'call AtomicBool);

impl Drop for ProcessFlagGuard<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

#[test]
fn borrowed_processor_flag_guard_clears_during_rust_only_unwind() {
    let active = AtomicBool::new(true);
    let result = std::panic::catch_unwind(|| {
        let _guard = ProcessFlagGuard(&active);
        panic!("guard test: Rust only, never through a COM vtable");
    });
    assert!(result.is_err());
    assert!(!active.load(Ordering::Acquire));
}
