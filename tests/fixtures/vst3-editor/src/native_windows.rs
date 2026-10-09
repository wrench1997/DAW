//! Citrus additions to the MIT upstream fixture: native drawing and real editor callbacks.
//! This code is developer-only and is never linked into the DAW or its helper.

use super::*;
use std::io::Write;
use std::mem;
use winapi::shared::minwindef::{HINSTANCE, LPARAM, LRESULT, UINT, WPARAM};
use winapi::shared::windef::{HBRUSH, HWND};
use winapi::um::fileapi::WriteFile;
use winapi::um::libloaderapi::{
    GetModuleHandleExW, GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS,
    GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
};
use winapi::um::processenv::GetStdHandle;
use winapi::um::winbase::STD_OUTPUT_HANDLE;
use winapi::um::winuser::*;

const BUTTON_ID: usize = 4101;
const CLASS_NAME: &str = "CitrusVst3FixturePanelV1";

// Each line is a valid HostResponse, so a host cannot pass this check merely by
// dropping unparseable logging. All three routes must be redirected to stderr
// before loading the fixture. These calls intentionally do not use eprintln!.
fn exercise_stdout_routes() -> bool {
    let rust = b"{\"Error\":{\"message\":\"CITRUS_FIXTURE_STDOUT_RUST\"}}\n";
    let win32 = b"{\"Error\":{\"message\":\"CITRUS_FIXTURE_STDOUT_WIN32\"}}\n";
    {
        let mut stdout = std::io::stdout().lock();
        if stdout.write_all(rust).is_err() || stdout.flush().is_err() {
            return false;
        }
    }
    unsafe {
        let handle = GetStdHandle(STD_OUTPUT_HANDLE);
        let mut written = 0;
        if handle.is_null()
            || handle as isize == -1
            || WriteFile(
                handle,
                win32.as_ptr().cast(),
                win32.len() as u32,
                &mut written,
                ptr::null_mut(),
            ) == 0
            || written != win32.len() as u32
        {
            return false;
        }
        // Exercise the fixture's C runtime as well as Rust and the Win32 standard
        // handle. No new runtime dependency is introduced: this is the target CRT.
        unsafe extern "C" {
            fn puts(text: *const c_char) -> i32;
            fn fflush(stream: *mut c_void) -> i32;
        }
        puts(c"{\"Error\":{\"message\":\"CITRUS_FIXTURE_STDOUT_CRT\"}}".as_ptr()) >= 0
            && fflush(ptr::null_mut()) == 0
    }
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(Some(0)).collect()
}

/// The callback data stays alive until DestroyWindow has returned. HWND and HINSTANCE are
/// stored as integers solely so the COM view can satisfy its Send/Sync marker requirements.
/// The host must call all view methods on the creating GUI thread, as VST3 requires.
pub(super) struct NativeWindow {
    hwnd: usize,
    module: usize,
    _edit: Box<NativeEditState>,
}

impl NativeWindow {
    pub(super) unsafe fn attach(
        parent: *mut c_void,
        size: (i32, i32),
        edit: NativeEditState,
    ) -> Option<Self> {
        if !exercise_stdout_routes() {
            eprintln!("Fixture stdout-routing probe could not write all three routes");
            return None;
        }
        let mut module: HINSTANCE = ptr::null_mut();
        if GetModuleHandleExW(
            GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
            window_proc as *const () as *const u16,
            &mut module,
        ) == 0
        {
            return None;
        }
        let class = wide(CLASS_NAME);
        let wc = WNDCLASSW {
            lpfnWndProc: Some(window_proc),
            hInstance: module,
            hCursor: LoadCursorW(ptr::null_mut(), IDC_ARROW),
            hbrBackground: (COLOR_WINDOW + 1) as HBRUSH,
            lpszClassName: class.as_ptr(),
            ..mem::zeroed()
        };
        if RegisterClassW(&wc) == 0 {
            return None;
        }
        let mut edit = Box::new(edit);
        let hwnd = CreateWindowExW(
            0,
            class.as_ptr(),
            wide("Citrus source-built VST3 fixture").as_ptr(),
            WS_CHILD | WS_VISIBLE | WS_CLIPCHILDREN,
            0,
            0,
            size.0,
            size.1,
            parent as HWND,
            ptr::null_mut(),
            module,
            (&mut *edit as *mut NativeEditState).cast(),
        );
        if hwnd.is_null() {
            UnregisterClassW(class.as_ptr(), module);
            return None;
        }
        let control = CreateWindowExW(
            0,
            wide("BUTTON").as_ptr(),
            wide("Set Cutoff to 0.25").as_ptr(),
            WS_CHILD | WS_VISIBLE | WS_TABSTOP | BS_PUSHBUTTON,
            24,
            24,
            256,
            48,
            hwnd,
            BUTTON_ID as _,
            module,
            ptr::null_mut(),
        );
        let window = Self {
            hwnd: hwnd as usize,
            module: module as usize,
            _edit: edit,
        };
        if control.is_null() {
            drop(window);
            return None;
        }
        // The standard Windows button draws its caption/border and implements BM_CLICK.
        // The acceptance harness verifies non-uniform painted pixels as well as callbacks.
        UpdateWindow(hwnd);
        Some(window)
    }

    pub(super) unsafe fn resize(&self, width: i32, height: i32) {
        MoveWindow(self.hwnd as HWND, 0, 0, width, height, 1);
    }
}

impl Drop for NativeWindow {
    fn drop(&mut self) {
        unsafe {
            if IsWindow(self.hwnd as HWND) != 0 && DestroyWindow(self.hwnd as HWND) == 0 {
                // A host violating GUI-thread ownership must not leave a live callback
                // pointing at freed Rust data or an unloaded DLL. Fail inside the helper.
                std::process::abort();
            }
            UnregisterClassW(wide(CLASS_NAME).as_ptr(), self.module as HINSTANCE);
        }
    }
}

unsafe extern "system" fn window_proc(hwnd: HWND, msg: UINT, w: WPARAM, l: LPARAM) -> LRESULT {
    // No Rust panic may unwind through the Win32 callback boundary.
    std::panic::catch_unwind(|| dispatch(hwnd, msg, w, l))
        .unwrap_or_else(|_| DefWindowProcW(hwnd, msg, w, l))
}

unsafe fn dispatch(hwnd: HWND, msg: UINT, w: WPARAM, l: LPARAM) -> LRESULT {
    if msg == WM_NCCREATE {
        let create = &*(l as *const CREATESTRUCTW);
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, create.lpCreateParams as isize);
    }
    if msg == WM_COMMAND && (w & 0xffff) == BUTTON_ID && ((w >> 16) & 0xffff) == BN_CLICKED as usize
    {
        let edit = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const NativeEditState;
        if let Some(edit) = edit.as_ref() {
            // Never hold a fixture mutex while calling back into the host.
            edit.values.lock().unwrap_or_else(|p| p.into_inner())[CUTOFF_PARAM_ID as usize] = 0.25;
            {
                let mut revision = edit.revision.lock().unwrap_or_else(|p| p.into_inner());
                *revision = revision.wrapping_add(1);
            }
            let handler = edit
                .handler
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .clone();
            if let Some(handler) = handler {
                handler.beginEdit(CUTOFF_PARAM_ID);
                handler.performEdit(CUTOFF_PARAM_ID, 0.25);
                handler.endEdit(CUTOFF_PARAM_ID);
                if let Some(handler2) = handler.cast::<IComponentHandler2>() {
                    handler2.setDirty(1);
                }
            }
            SetWindowTextW(
                GetDlgItem(hwnd, BUTTON_ID as i32),
                wide("Cutoff = 0.25 (edited)").as_ptr(),
            );
        }
        return 0;
    }
    if msg == WM_NCDESTROY {
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
    }
    DefWindowProcW(hwnd, msg, w, l)
}
