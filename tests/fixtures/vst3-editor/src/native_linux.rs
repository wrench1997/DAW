//! Source-built Linux XEmbed fixture. Drawing, fd delivery, timers and input all use the
//! host's IRunLoop; no fixture-owned event thread can hide a missing host run loop.
use super::*;
use std::os::fd::AsRawFd;
use vst3::Steinberg::Linux::*;
use xcb::{x, Xid, XidNew};

fn exercise_stdout_routes() -> bool {
    use std::io::Write;
    if std::io::stdout()
        .write_all(b"{\"Error\":{\"message\":\"CITRUS_FIXTURE_STDOUT_RUST\"}}\n")
        .is_err()
        || std::io::stdout().flush().is_err()
    {
        return false;
    }
    unsafe extern "C" {
        fn write(fd: i32, data: *const c_void, size: usize) -> isize;
        fn puts(text: *const c_char) -> i32;
        fn fflush(stream: *mut c_void) -> i32;
    }
    let direct = b"{\"Error\":{\"message\":\"CITRUS_FIXTURE_STDOUT_POSIX\"}}\n";
    unsafe {
        write(1, direct.as_ptr().cast(), direct.len()) == direct.len() as isize
            && puts(c"{\"Error\":{\"message\":\"CITRUS_FIXTURE_STDOUT_CRT\"}}".as_ptr()) >= 0
            && fflush(ptr::null_mut()) == 0
    }
}

static FACTORY_FD: AtomicU32 = AtomicU32::new(0);
static FACTORY_TIMER: AtomicU32 = AtomicU32::new(0);
static FRAME_FD: AtomicU32 = AtomicU32::new(0);
static FRAME_TIMER: AtomicU32 = AtomicU32::new(0);
static KEY_PRESS: AtomicU32 = AtomicU32::new(0);
static KEY_RELEASE: AtomicU32 = AtomicU32::new(0);
static MOUSE_PRESS: AtomicU32 = AtomicU32::new(0);
pub(super) fn probe(id: u32) -> Option<f64> {
    let counter = match id {
        1020 => &FACTORY_FD,
        1021 => &FACTORY_TIMER,
        1022 => &FRAME_FD,
        1023 => &FRAME_TIMER,
        1024 => &KEY_PRESS,
        1025 => &KEY_RELEASE,
        1026 => &MOUSE_PRESS,
        _ => return None,
    };
    Some(f64::from(counter.load(Ordering::SeqCst).min(1_000_000)) / 1_000_000.0)
}

struct Surface {
    connection: xcb::Connection,
    window: x::Window,
    gc: x::Gcontext,
    edit: NativeEditState,
    changed: bool,
    first_timer: bool,
    thread: std::thread::ThreadId,
}

impl Surface {
    fn paint(&self) {
        self.connection.send_request(&x::ChangeGc {
            gc: self.gc,
            value_list: &[x::Gc::Foreground(0x172631)],
        });
        self.connection.send_request(&x::PolyFillRectangle {
            drawable: x::Drawable::Window(self.window),
            gc: self.gc,
            rectangles: &[x::Rectangle {
                x: 0,
                y: 0,
                width: 4096,
                height: 4096,
            }],
        });
        self.connection.send_request(&x::ChangeGc {
            gc: self.gc,
            value_list: &[x::Gc::Foreground(if self.changed {
                0x53bd81
            } else {
                0xffaf3f
            })],
        });
        self.connection.send_request(&x::PolyFillRectangle {
            drawable: x::Drawable::Window(self.window),
            gc: self.gc,
            rectangles: &[x::Rectangle {
                x: 24,
                y: 24,
                width: 300,
                height: 64,
            }],
        });
        self.connection.send_request(&x::ChangeGc {
            gc: self.gc,
            value_list: &[
                x::Gc::Foreground(0x101820),
                x::Gc::Background(if self.changed { 0x53bd81 } else { 0xffaf3f }),
            ],
        });
        self.connection.send_request(&x::ImageText8 {
            drawable: x::Drawable::Window(self.window),
            gc: self.gc,
            x: 38,
            y: 60,
            string: if self.changed {
                b"Cutoff set to 0.25"
            } else {
                b"Click: set Cutoff to 0.25"
            },
        });
        self.connection.send_request(&x::ChangeGc {
            gc: self.gc,
            value_list: &[x::Gc::Foreground(0xffffff), x::Gc::Background(0x172631)],
        });
        self.connection.send_request(&x::ImageText8 {
            drawable: x::Drawable::Window(self.window),
            gc: self.gc,
            x: 24,
            y: 130,
            string: b"Citrus Linux source-built VST3 fixture",
        });
        self.connection.send_request(&x::ImageText8 {
            drawable: x::Drawable::Window(self.window),
            gc: self.gc,
            x: 24,
            y: 160,
            string: b"Host IRunLoop: fd + timer on main thread",
        });
        let _ = self.connection.flush();
    }
    fn edit(&mut self) {
        self.changed = true;
        self.edit.values.lock().unwrap()[CUTOFF_PARAM_ID as usize] = 0.25;
        *self.edit.revision.lock().unwrap() += 1;
        let handler = self.edit.handler.lock().unwrap().clone();
        if let Some(handler) = handler {
            unsafe {
                handler.beginEdit(CUTOFF_PARAM_ID);
                handler.performEdit(CUTOFF_PARAM_ID, 0.25);
                handler.endEdit(CUTOFF_PARAM_ID);
                if let Some(handler2) = handler.cast::<IComponentHandler2>() {
                    handler2.setDirty(1);
                }
            }
        }
        eprintln!("CITRUS_LINUX_FIXTURE_ACTUAL_INPUT cutoff=0.25");
        self.paint();
    }
    fn poll(&mut self) {
        assert_eq!(
            self.thread,
            std::thread::current().id(),
            "fixture UI escaped main thread"
        );
        for _ in 0..32 {
            let Ok(Some(xcb::Event::X(event))) = self.connection.poll_for_event() else {
                break;
            };
            match event {
                x::Event::Expose(_) => self.paint(),
                x::Event::ButtonPress(event)
                    if event.detail() == 1
                        && (24..324).contains(&event.event_x())
                        && (24..88).contains(&event.event_y()) =>
                {
                    MOUSE_PRESS.fetch_add(1, Ordering::SeqCst);
                    self.edit()
                }
                x::Event::KeyPress(_) => {
                    KEY_PRESS.fetch_add(1, Ordering::SeqCst);
                    self.edit();
                }
                x::Event::KeyRelease(_) => {
                    KEY_RELEASE.fetch_add(1, Ordering::SeqCst);
                }
                _ => {}
            }
        }
    }
}

struct Pump {
    surface: Arc<Mutex<Surface>>,
}
impl Class for Pump {
    type Interfaces = (IEventHandler, ITimerHandler);
}
impl IEventHandlerTrait for Pump {
    unsafe fn onFDIsSet(&self, _fd: FileDescriptor) {
        FRAME_FD.fetch_add(1, Ordering::SeqCst);
        self.surface.lock().unwrap().poll();
    }
}
impl ITimerHandlerTrait for Pump {
    unsafe fn onTimer(&self) {
        FRAME_TIMER.fetch_add(1, Ordering::SeqCst);
        let mut surface = self.surface.lock().unwrap();
        assert_eq!(surface.thread, std::thread::current().id());
        if !surface.first_timer {
            surface.first_timer = true;
            eprintln!("CITRUS_LINUX_FIXTURE_TIMER main_thread=true");
            surface.paint();
        }
    }
}

pub(super) struct NativeWindow {
    surface: Arc<Mutex<Surface>>,
    run_loop: vst3::ComPtr<IRunLoop>,
    event_handler: vst3::ComPtr<IEventHandler>,
    timer_handler: vst3::ComPtr<ITimerHandler>,
}
impl NativeWindow {
    pub(super) unsafe fn attach(
        parent: *mut c_void,
        size: (i32, i32),
        edit: NativeEditState,
        frame: &vst3::ComPtr<IPlugFrame>,
    ) -> Option<Self> {
        if !exercise_stdout_routes() {
            return None;
        }
        let run_loop = frame.cast::<IRunLoop>()?;
        let (connection, screen) = xcb::Connection::connect(None).ok()?;
        let visual = connection
            .get_setup()
            .roots()
            .nth(screen as usize)?
            .root_visual();
        let window = connection.generate_id();
        let parent = x::Window::new(u32::try_from(parent as usize).ok()?);
        connection
            .send_and_check_request(&x::CreateWindow {
                depth: x::COPY_FROM_PARENT as u8,
                wid: window,
                parent,
                x: 0,
                y: 0,
                width: size.0 as u16,
                height: size.1 as u16,
                border_width: 0,
                class: x::WindowClass::InputOutput,
                visual,
                value_list: &[x::Cw::EventMask(
                    x::EventMask::EXPOSURE
                        | x::EventMask::BUTTON_PRESS
                        | x::EventMask::KEY_PRESS
                        | x::EventMask::KEY_RELEASE,
                )],
            })
            .ok()?;
        let atom = connection
            .wait_for_reply(connection.send_request(&x::InternAtom {
                only_if_exists: false,
                name: b"_XEMBED_INFO",
            }))
            .ok()?
            .atom();
        connection.send_request(&x::ChangeProperty {
            mode: x::PropMode::Replace,
            window,
            property: atom,
            r#type: atom,
            data: &[0u32, 1],
        });
        let font: x::Font = connection.generate_id();
        connection
            .send_and_check_request(&x::OpenFont {
                fid: font,
                name: b"fixed",
            })
            .ok()?;
        let gc = connection.generate_id();
        connection
            .send_and_check_request(&x::CreateGc {
                cid: gc,
                drawable: x::Drawable::Window(window),
                value_list: &[
                    x::Gc::Foreground(0xffffff),
                    x::Gc::Background(0x172631),
                    x::Gc::Font(font),
                ],
            })
            .ok()?;
        connection.send_request(&x::MapWindow { window });
        connection.flush().ok()?;
        let fd = connection.as_raw_fd();
        let surface = Arc::new(Mutex::new(Surface {
            connection,
            window,
            gc,
            edit,
            changed: false,
            first_timer: false,
            thread: std::thread::current().id(),
        }));
        let pump = ComWrapper::new(Pump {
            surface: surface.clone(),
        });
        let event_handler = pump.to_com_ptr::<IEventHandler>()?;
        let timer_handler = pump.to_com_ptr::<ITimerHandler>()?;
        if run_loop.registerEventHandler(event_handler.as_ptr(), fd) != kResultOk {
            return None;
        }
        if run_loop.registerTimer(timer_handler.as_ptr(), 25) != kResultOk {
            run_loop.unregisterEventHandler(event_handler.as_ptr());
            return None;
        }
        surface.lock().unwrap().paint();
        eprintln!(
            "CITRUS_LINUX_FIXTURE_ATTACH xid={}",
            surface.lock().unwrap().window.resource_id()
        );
        Some(Self {
            surface,
            run_loop,
            event_handler,
            timer_handler,
        })
    }
    pub(super) fn resize(&self, width: i32, height: i32) {
        let surface = self.surface.lock().unwrap();
        surface.connection.send_request(&x::ConfigureWindow {
            window: surface.window,
            value_list: &[
                x::ConfigWindow::Width(width as u32),
                x::ConfigWindow::Height(height as u32),
            ],
        });
        let _ = surface.connection.flush();
    }
}
impl Drop for NativeWindow {
    fn drop(&mut self) {
        unsafe {
            self.run_loop
                .unregisterEventHandler(self.event_handler.as_ptr());
            self.run_loop.unregisterTimer(self.timer_handler.as_ptr());
        }
        let surface = self.surface.lock().unwrap();
        let _ = surface
            .connection
            .send_and_check_request(&x::DestroyWindow {
                window: surface.window,
            });
        let _ = surface.connection.flush();
        eprintln!("CITRUS_LINUX_FIXTURE_DETACH");
    }
}

// A second, factory-context registration deliberately outlives editor Close. This catches
// hosts that expose IRunLoop only on IPlugFrame or clear all registrations on view detach.
struct FactoryCallbacks {
    read: Mutex<std::os::unix::net::UnixStream>,
    thread: std::thread::ThreadId,
    ticks: AtomicU32,
}
impl Class for FactoryCallbacks {
    type Interfaces = (IEventHandler, ITimerHandler);
}
impl IEventHandlerTrait for FactoryCallbacks {
    unsafe fn onFDIsSet(&self, _fd: FileDescriptor) {
        use std::io::Read;
        assert_eq!(self.thread, std::thread::current().id());
        let mut buffer = [0u8; 1];
        if self.read.lock().unwrap().read(&mut buffer).ok() == Some(1) {
            FACTORY_FD.fetch_add(1, Ordering::SeqCst);
            eprintln!("CITRUS_LINUX_FACTORY_FD main_thread=true");
        }
    }
}
impl ITimerHandlerTrait for FactoryCallbacks {
    unsafe fn onTimer(&self) {
        assert_eq!(self.thread, std::thread::current().id());
        FACTORY_TIMER.fetch_add(1, Ordering::SeqCst);
        if self.ticks.fetch_add(1, Ordering::Relaxed) == 0 {
            eprintln!("CITRUS_LINUX_FACTORY_TIMER main_thread=true");
        }
    }
}
pub(super) struct FactoryProbe {
    run_loop: vst3::ComPtr<IRunLoop>,
    event: vst3::ComPtr<IEventHandler>,
    timer: vst3::ComPtr<ITimerHandler>,
    _write: std::os::unix::net::UnixStream,
}
impl FactoryProbe {
    pub(super) unsafe fn new(context: *mut FUnknown) -> Option<Self> {
        use std::io::Write;
        let run_loop = ComRef::from_raw(context)?.cast::<IRunLoop>()?;
        let (read, mut write) = std::os::unix::net::UnixStream::pair().ok()?;
        read.set_nonblocking(true).ok()?;
        let fd = read.as_raw_fd();
        let callbacks = ComWrapper::new(FactoryCallbacks {
            read: Mutex::new(read),
            thread: std::thread::current().id(),
            ticks: AtomicU32::new(0),
        });
        let event = callbacks.to_com_ptr::<IEventHandler>()?;
        let timer = callbacks.to_com_ptr::<ITimerHandler>()?;
        if run_loop.registerEventHandler(event.as_ptr(), fd) != kResultOk {
            return None;
        }
        if run_loop.registerTimer(timer.as_ptr(), 25) != kResultOk {
            run_loop.unregisterEventHandler(event.as_ptr());
            return None;
        }
        write.write_all(&[1]).ok()?;
        Some(Self {
            run_loop,
            event,
            timer,
            _write: write,
        })
    }
}
impl Drop for FactoryProbe {
    fn drop(&mut self) {
        unsafe {
            self.run_loop.unregisterEventHandler(self.event.as_ptr());
            self.run_loop.unregisterTimer(self.timer.as_ptr());
        }
    }
}
