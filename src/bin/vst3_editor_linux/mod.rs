//! Standalone X11 plugin containers, including when DISPLAY belongs to system XWayland.
//! The DAW's Wayland surface is never used as an XID. All plugin calls and XCB work run
//! on the helper main thread; only bounded stdin parsing runs in another thread.

use crate::{SharedPlugin, err, handle, respond};
use std::io::{self, BufRead};
use std::sync::mpsc::{self, SyncSender, TryRecvError};
use std::time::Duration;
use vst3_host::process_isolation::{HostCommand, HostResponse, ProtocolChannel};
use vst3_host::{IsolatedEditorCommand, IsolatedEditorOwner, IsolatedEditorState, WindowHandle};
use xcb::{Raw, Xid, x};

const COMMAND_QUEUE_CAPACITY: usize = 4;
const COMMAND_BATCH: usize = 4;
const EVENT_BATCH: usize = 32;
const MAX_COMMAND_BYTES: usize = 96 * 1024 * 1024;
const MAX_EDITOR_DIMENSION: i32 = 16_384;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Size {
    width: i32,
    height: i32,
}
impl Size {
    fn checked(width: i32, height: i32) -> Result<Self, String> {
        if !(1..=MAX_EDITOR_DIMENSION).contains(&width)
            || !(1..=MAX_EDITOR_DIMENSION).contains(&height)
        {
            return Err(format!(
                "Editor size {width}x{height} is outside 1..={MAX_EDITOR_DIMENSION}"
            ));
        }
        Ok(Self { width, height })
    }
}

fn requires_editor_detach(command: &HostCommand) -> bool {
    matches!(
        command,
        HostCommand::LoadPlugin { .. }
            | HostCommand::UnloadPlugin
            | HostCommand::SaveState
            | HostCommand::LoadState { .. }
    )
}

fn validate_owner(owner: Option<IsolatedEditorOwner>) -> Result<(), String> {
    if owner.is_some() {
        Err("Linux plugin windows are standalone; a Windows HWND owner is invalid".into())
    } else {
        Ok(())
    }
}

// X11 exposes a desktop-wide Xft resource, not a reliable Wayland per-output scale.
// Do not multiply this by the DAW's scale or guess a Wayland surface's physical DPI.
fn resource_scale(resources: &[u8]) -> f32 {
    std::str::from_utf8(resources)
        .ok()
        .and_then(|text| {
            text.lines().find_map(|line| {
                let (key, value) = line.split_once(':')?;
                if key.trim() != "Xft.dpi" {
                    return None;
                }
                value
                    .trim()
                    .parse::<f32>()
                    .ok()
                    .filter(|dpi| dpi.is_finite() && (48.0..=384.0).contains(dpi))
                    .map(|dpi| dpi / 96.0)
            })
        })
        .unwrap_or(1.0)
}

#[derive(Default)]
struct Signals {
    close: bool,
    destroyed: bool,
    size: Option<Size>,
    scale_changed: bool,
}

struct Atoms {
    protocols: x::Atom,
    delete: x::Atom,
    take_focus: x::Atom,
    active: x::Atom,
    xembed: x::Atom,
    xembed_info: x::Atom,
    resources: x::Atom,
    utf8: x::Atom,
    name: x::Atom,
    pid: x::Atom,
}

impl Atoms {
    fn new(connection: &xcb::Connection) -> Result<Self, String> {
        let atom = |name: &[u8]| {
            connection
                .wait_for_reply(connection.send_request(&x::InternAtom {
                    only_if_exists: false,
                    name,
                }))
                .map(|reply| reply.atom())
                .map_err(|e| e.to_string())
        };
        Ok(Self {
            protocols: atom(b"WM_PROTOCOLS")?,
            delete: atom(b"WM_DELETE_WINDOW")?,
            take_focus: atom(b"WM_TAKE_FOCUS")?,
            active: atom(b"_NET_ACTIVE_WINDOW")?,
            xembed: atom(b"_XEMBED")?,
            xembed_info: atom(b"_XEMBED_INFO")?,
            resources: atom(b"RESOURCE_MANAGER")?,
            utf8: atom(b"UTF8_STRING")?,
            name: atom(b"_NET_WM_NAME")?,
            pid: atom(b"_NET_WM_PID")?,
        })
    }
}

struct NativeWindow {
    connection: xcb::Connection,
    window: x::Window,
    root: x::Window,
    focus_proxy: x::Window,
    active: bool,
    focused: bool,
    atoms: Atoms,
    embedded: Option<x::Window>,
    embedded_mapped: bool,
    size: Size,
    resizable: bool,
    alive: bool,
    signals: Signals,
}

impl NativeWindow {
    fn new(title: &str, size: Size) -> Result<Self, String> {
        let (connection, number) = xcb::Connection::connect(None).map_err(|e| format!("No usable X11/XWayland display: {e}. The desktop must provide DISPLAY; Citrus does not start a display server."))?;
        let screen = connection
            .get_setup()
            .roots()
            .nth(number as usize)
            .ok_or("No X11 screen")?;
        let root = screen.root();
        let window = connection.generate_id();
        let atoms = Atoms::new(&connection)?;
        connection
            .send_and_check_request(&x::CreateWindow {
                depth: x::COPY_FROM_PARENT as u8,
                wid: window,
                parent: root,
                x: 100,
                y: 100,
                width: size.width as u16,
                height: size.height as u16,
                border_width: 0,
                class: x::WindowClass::InputOutput,
                visual: screen.root_visual(),
                value_list: &[
                    x::Cw::BackPixel(screen.black_pixel()),
                    x::Cw::EventMask(
                        x::EventMask::STRUCTURE_NOTIFY
                            | x::EventMask::SUBSTRUCTURE_NOTIFY
                            | x::EventMask::FOCUS_CHANGE
                            | x::EventMask::PROPERTY_CHANGE,
                    ),
                ],
            })
            .map_err(|e| format!("Create X11 plugin window: {e}"))?;
        let focus_proxy = connection.generate_id();
        let result = Self {
            connection,
            window,
            root,
            focus_proxy,
            active: false,
            focused: false,
            atoms,
            embedded: None,
            embedded_mapped: false,
            size,
            resizable: false,
            alive: true,
            signals: Signals::default(),
        };
        result
            .connection
            .send_and_check_request(&x::CreateWindow {
                depth: 0,
                wid: focus_proxy,
                parent: window,
                x: -1,
                y: -1,
                width: 1,
                height: 1,
                border_width: 0,
                class: x::WindowClass::InputOnly,
                visual: x::COPY_FROM_PARENT,
                value_list: &[x::Cw::EventMask(
                    x::EventMask::KEY_PRESS
                        | x::EventMask::KEY_RELEASE
                        | x::EventMask::FOCUS_CHANGE,
                )],
            })
            .map_err(|e| e.to_string())?;
        result.connection.send_request(&x::MapWindow {
            window: focus_proxy,
        });
        result
            .connection
            .send_and_check_request(&x::ChangeWindowAttributes {
                window: root,
                value_list: &[x::Cw::EventMask(x::EventMask::PROPERTY_CHANGE)],
            })
            .map_err(|e| e.to_string())?;
        result.property(x::ATOM_WM_NAME, x::ATOM_STRING, title.as_bytes());
        result.property(result.atoms.name, result.atoms.utf8, title.as_bytes());
        result.property(
            x::ATOM_WM_CLASS,
            x::ATOM_STRING,
            b"citrus-vst3-editor\0CitrusVst3Editor\0",
        );
        result.property(result.atoms.pid, x::ATOM_CARDINAL, &[std::process::id()]);
        result.property(
            result.atoms.protocols,
            x::ATOM_ATOM,
            &[result.atoms.delete, result.atoms.take_focus],
        );
        // ICCCM globally-active model: the WM grants focus via timestamped WM_TAKE_FOCUS.
        result.property(
            x::ATOM_WM_HINTS,
            x::ATOM_WM_HINTS,
            &[1u32, 0, 0, 0, 0, 0, 0, 0, 0],
        );
        result.update_size_hints();
        result.connection.flush().map_err(|e| e.to_string())?;
        Ok(result)
    }

    fn property<T: x::PropEl>(&self, property: x::Atom, kind: x::Atom, data: &[T]) {
        self.connection.send_request(&x::ChangeProperty {
            mode: x::PropMode::Replace,
            window: self.window,
            property,
            r#type: kind,
            data,
        });
    }

    fn scale(&self) -> f32 {
        self.connection
            .wait_for_reply(self.connection.send_request(&x::GetProperty {
                delete: false,
                window: self.root,
                property: self.atoms.resources,
                r#type: x::ATOM_STRING,
                long_offset: 0,
                long_length: 16_384,
            }))
            .ok()
            .filter(|r| r.format() == 8)
            .map(|r| resource_scale(r.value::<u8>()))
            .unwrap_or(1.0)
    }

    fn update_size_hints(&self) {
        let (min_width, min_height, max_width, max_height) = if self.resizable {
            (1, 1, MAX_EDITOR_DIMENSION, MAX_EDITOR_DIMENSION)
        } else {
            (
                self.size.width,
                self.size.height,
                self.size.width,
                self.size.height,
            )
        };
        // ICCCM XSizeHints wire layout: PMinSize | PMaxSize, then x/y/w/h, bounds.
        self.property(
            x::ATOM_WM_NORMAL_HINTS,
            x::ATOM_WM_SIZE_HINTS,
            &[
                48u32,
                0,
                0,
                0,
                0,
                min_width as u32,
                min_height as u32,
                max_width as u32,
                max_height as u32,
                0,
                0,
                0,
                0,
                0,
                0,
                0,
                0,
                0,
            ],
        );
    }

    fn set_size(&mut self, size: Size) -> Result<(), String> {
        self.connection
            .send_and_check_request(&x::ConfigureWindow {
                window: self.window,
                value_list: &[
                    x::ConfigWindow::Width(size.width as u32),
                    x::ConfigWindow::Height(size.height as u32),
                ],
            })
            .map_err(|e| e.to_string())?;
        self.size = size;
        self.update_size_hints();
        self.connection.flush().map_err(|e| e.to_string())
    }

    fn send_embed(
        &self,
        child: x::Window,
        timestamp: u32,
        message: u32,
        detail: u32,
        data1: u32,
        data2: u32,
    ) {
        let event = x::ClientMessageEvent::new(
            child,
            self.atoms.xembed,
            x::ClientMessageData::Data32([timestamp, message, detail, data1, data2]),
        );
        self.connection.send_request(&x::SendEvent {
            propagate: false,
            destination: x::SendEventDest::Window(child),
            event_mask: x::EventMask::NO_EVENT,
            event: &event,
        });
    }

    fn discover_embedded(&mut self) -> Result<(), String> {
        let tree = self
            .connection
            .wait_for_reply(self.connection.send_request(&x::QueryTree {
                window: self.window,
            }))
            .map_err(|e| e.to_string())?;
        if self
            .embedded
            .is_some_and(|child| tree.children().contains(&child))
        {
            return Ok(());
        }
        self.embedded = None;
        self.embedded_mapped = false;
        // Only direct children of our own container are considered. Ignore temporary children
        // without the XEmbed property and never send protocol traffic to unrelated windows.
        for &child in tree.children().iter().take(128) {
            if child == self.focus_proxy {
                continue;
            }
            let Ok(info) =
                self.connection
                    .wait_for_reply(self.connection.send_request(&x::GetProperty {
                        delete: false,
                        window: child,
                        property: self.atoms.xembed_info,
                        r#type: self.atoms.xembed_info,
                        long_offset: 0,
                        long_length: 2,
                    }))
            else {
                continue;
            };
            if info.format() != 32 || info.value::<u32>().len() < 2 {
                continue;
            }
            match self
                .connection
                .send_and_check_request(&x::ChangeWindowAttributes {
                    window: child,
                    value_list: &[x::Cw::EventMask(
                        x::EventMask::PROPERTY_CHANGE | x::EventMask::STRUCTURE_NOTIFY,
                    )],
                }) {
                Ok(()) => {}
                Err(xcb::ProtocolError::X(x::Error::Window(_), _)) => continue,
                Err(error) => return Err(error.to_string()),
            }
            self.embedded = Some(child);
            self.embedded_mapped = info.value::<u32>()[1] & 1 != 0;
            self.send_embed(child, x::CURRENT_TIME, 0, 0, self.window.resource_id(), 0); // EMBEDDED_NOTIFY, version 0
            if self.embedded_mapped {
                self.connection
                    .send_request(&x::MapWindow { window: child });
            } else {
                self.connection
                    .send_request(&x::UnmapWindow { window: child });
            }
            if self.active {
                self.send_embed(child, x::CURRENT_TIME, 1, 0, 0, 0);
            }
            if self.active && self.focused && self.embedded_mapped {
                self.send_embed(child, x::CURRENT_TIME, 4, 0, 0, 0);
            }
            break;
        }
        Ok(())
    }

    fn focus_child(&self, timestamp: u32) {
        // Client focus requests never authorize activating an inactive top-level.
        if !self.active || !self.embedded_mapped || timestamp == x::CURRENT_TIME {
            return;
        }
        self.connection.send_request(&x::SetInputFocus {
            revert_to: x::InputFocus::Parent,
            focus: self.focus_proxy,
            time: timestamp,
        });
        if let Some(child) = self.embedded {
            self.send_embed(child, timestamp, 4, 0, 0, 0);
        }
    }

    fn forward_key(&self, event: &x::KeyPressEvent, release: bool) {
        if !self.active || !self.focused || !self.embedded_mapped {
            return;
        }
        let Some(child) = self.embedded else {
            return;
        };
        let forwarded = x::KeyPressEvent::new(
            event.detail(),
            event.time(),
            self.root,
            child,
            x::WINDOW_NONE,
            event.root_x(),
            event.root_y(),
            0,
            0,
            event.state(),
            event.same_screen(),
        );
        // XCB aliases KeyReleaseEvent to KeyPressEvent, whose constructor always writes 2.
        // The owned 32-byte X11 key event is otherwise identical; set its protocol opcode.
        unsafe {
            (*forwarded.as_raw()).response_type = if release { 3 } else { 2 };
        }
        self.connection.send_request(&x::SendEvent {
            propagate: false,
            destination: x::SendEventDest::Window(child),
            event_mask: x::EventMask::NO_EVENT,
            event: &forwarded,
        });
    }

    fn focus(&self) -> Result<(), String> {
        self.connection
            .send_and_check_request(&x::MapWindow {
                window: self.window,
            })
            .map_err(|e| e.to_string())?;
        self.connection.send_request(&x::ConfigureWindow {
            window: self.window,
            value_list: &[x::ConfigWindow::StackMode(x::StackMode::Above)],
        });
        // A compositor may decline activation (focus stealing prevention). This is a request,
        // not a promise of Wayland cross-process focus/ownership.
        let event = x::ClientMessageEvent::new(
            self.window,
            self.atoms.active,
            x::ClientMessageData::Data32([1, x::CURRENT_TIME, 0, 0, 0]),
        );
        self.connection.send_request(&x::SendEvent {
            propagate: false,
            destination: x::SendEventDest::Window(self.root),
            event_mask: x::EventMask::SUBSTRUCTURE_NOTIFY | x::EventMask::SUBSTRUCTURE_REDIRECT,
            event: &event,
        });
        self.connection.flush().map_err(|e| e.to_string())
    }

    fn poll(&mut self) -> Result<(), String> {
        for _ in 0..EVENT_BATCH {
            let event = match self.connection.poll_for_event() {
                Ok(Some(event)) => event,
                Ok(None) => break,
                Err(xcb::Error::Protocol(xcb::ProtocolError::X(x::Error::Window(error), _)))
                    if ![
                        self.window.resource_id(),
                        self.focus_proxy.resource_id(),
                        self.root.resource_id(),
                    ]
                    .contains(&error.bad_value()) =>
                {
                    // Plugins can rebuild their own child asynchronously. An in-flight
                    // XEmbed/key/map message to the retired child is not a display failure.
                    if self
                        .embedded
                        .is_some_and(|child| child.resource_id() == error.bad_value())
                    {
                        self.embedded = None;
                        self.embedded_mapped = false;
                    }
                    self.discover_embedded()?;
                    continue;
                }
                Err(error) => return Err(error.to_string()),
            };
            let xcb::Event::X(event) = event else {
                continue;
            };
            match event {
                x::Event::ClientMessage(event) if event.window() == self.window => {
                    if let x::ClientMessageData::Data32(data) = event.data() {
                        if event.r#type() == self.atoms.protocols {
                            if data[0] == self.atoms.delete.resource_id() {
                                self.signals.close = true;
                            }
                            if data[0] == self.atoms.take_focus.resource_id() {
                                self.connection.send_request(&x::SetInputFocus {
                                    revert_to: x::InputFocus::Parent,
                                    focus: self.focus_proxy,
                                    time: data[1],
                                });
                            }
                        } else if event.r#type() == self.atoms.xembed && data[1] == 3 {
                            if self.active && data[0] != x::CURRENT_TIME {
                                self.focus_child(data[0]);
                            } else {
                                self.focus()?;
                            }
                        }
                    }
                }
                x::Event::ConfigureNotify(event) if event.window() == self.window => {
                    self.signals.size = Some(Size::checked(
                        i32::from(event.width()),
                        i32::from(event.height()),
                    )?);
                }
                x::Event::DestroyNotify(event) if event.window() == self.window => {
                    self.alive = false;
                    self.signals.destroyed = true;
                }
                x::Event::DestroyNotify(event) if Some(event.window()) == self.embedded => {
                    self.embedded = None;
                    self.embedded_mapped = false;
                }
                x::Event::ReparentNotify(event)
                    if Some(event.window()) == self.embedded && event.parent() != self.window =>
                {
                    self.embedded = None;
                    self.embedded_mapped = false;
                    self.discover_embedded()?;
                }
                x::Event::CreateNotify(_)
                | x::Event::ReparentNotify(_)
                | x::Event::MapNotify(_) => {
                    self.discover_embedded()?;
                }
                x::Event::FocusIn(event)
                    if event.event() == self.window
                        && event.detail() != x::NotifyDetail::Inferior =>
                {
                    self.active = true;
                    if let Some(child) = self.embedded {
                        self.send_embed(child, x::CURRENT_TIME, 1, 0, 0, 0);
                    }
                    // A queued FocusIn may already be stale. Only timestamped WM_TAKE_FOCUS
                    // transfers X input focus; never steal it back using CURRENT_TIME.
                }
                x::Event::FocusIn(event) if event.event() == self.focus_proxy => {
                    self.focused = true;
                    if self.active
                        && self.embedded_mapped
                        && let Some(child) = self.embedded
                    {
                        self.send_embed(child, x::CURRENT_TIME, 4, 0, 0, 0);
                    }
                }
                x::Event::FocusOut(event)
                    if event.event() == self.window
                        && event.detail() != x::NotifyDetail::Inferior =>
                {
                    self.active = false;
                    self.focused = false;
                    if let Some(child) = self.embedded {
                        self.send_embed(child, x::CURRENT_TIME, 5, 0, 0, 0);
                        self.send_embed(child, x::CURRENT_TIME, 2, 0, 0, 0);
                    }
                }
                x::Event::KeyPress(event) if event.event() == self.focus_proxy => {
                    self.forward_key(&event, false)
                }
                x::Event::KeyRelease(event) if event.event() == self.focus_proxy => {
                    self.forward_key(&event, true)
                }
                x::Event::PropertyNotify(event)
                    if event.window() == self.root && event.atom() == self.atoms.resources =>
                {
                    self.signals.scale_changed = true;
                }
                x::Event::PropertyNotify(event)
                    if Some(event.window()) == self.embedded
                        && event.atom() == self.atoms.xembed_info =>
                {
                    // XEMBED_MAPPED is maintained by the client. Honor changes without
                    // inventing a new editor generation or destroying the container.
                    if let Ok(info) = self.connection.wait_for_reply(self.connection.send_request(
                        &x::GetProperty {
                            delete: false,
                            window: event.window(),
                            property: self.atoms.xembed_info,
                            r#type: self.atoms.xembed_info,
                            long_offset: 0,
                            long_length: 2,
                        },
                    )) && info.format() == 32
                        && info.value::<u32>().len() == 2
                    {
                        self.embedded_mapped = info.value::<u32>()[1] & 1 != 0;
                        if self.embedded_mapped {
                            self.connection.send_request(&x::MapWindow {
                                window: event.window(),
                            });
                        } else {
                            self.connection.send_request(&x::UnmapWindow {
                                window: event.window(),
                            });
                        }
                        if self.active && self.focused {
                            self.send_embed(
                                event.window(),
                                event.time(),
                                if self.embedded_mapped { 4 } else { 5 },
                                0,
                                0,
                                0,
                            );
                        }
                    }
                }
                _ => {}
            }
        }
        self.connection.flush().map_err(|e| e.to_string())
    }

    fn destroy(&mut self) -> Result<(), String> {
        if self.alive {
            self.connection
                .send_and_check_request(&x::DestroyWindow {
                    window: self.window,
                })
                .map_err(|e| e.to_string())?;
            self.alive = false;
            self.connection.flush().map_err(|e| e.to_string())?;
        }
        Ok(())
    }
}

impl Drop for NativeWindow {
    fn drop(&mut self) {
        if let Err(error) = self.destroy() {
            fatal(&format!("destroying native container: {error}"));
        }
    }
}

fn fatal(message: &str) -> ! {
    eprintln!("Fatal Linux native editor lifecycle: {message}; terminating isolated helper");
    std::process::exit(1)
}

struct Editor {
    plugin: SharedPlugin,
    window: Option<NativeWindow>,
    state: IsolatedEditorState,
}
impl Editor {
    fn new(plugin: SharedPlugin) -> Self {
        Self {
            plugin,
            window: None,
            state: IsolatedEditorState {
                supported: true,
                has_editor: false,
                open: false,
                width: 0,
                height: 0,
                generation: 0,
            },
        }
    }
    fn plugin_changed(&mut self) {
        self.state.has_editor = self
            .plugin
            .lock()
            .ok()
            .and_then(|p| p.as_ref().map(|p| p.has_editor()))
            .unwrap_or(false);
    }
    fn open(&mut self, owner: Option<IsolatedEditorOwner>) -> Result<(), String> {
        validate_owner(owner)?;
        if let Some(window) = &self.window {
            return window.focus();
        }
        let mut guard = self.plugin.lock().map_err(|_| "plugin lock poisoned")?;
        let plugin = guard.as_mut().ok_or("No plugin loaded")?;
        if !self.state.has_editor {
            return Err("Plugin does not have a GUI editor".into());
        }
        let (width, height) = plugin.get_editor_size().map_err(|e| e.to_string())?;
        let mut window = NativeWindow::new(
            &format!("{} - VST3", plugin.info().name),
            Size::checked(width, height)?,
        )?;
        let _ = plugin.take_editor_resize_request();
        plugin
            .set_editor_scale_factor(window.scale())
            .map_err(|e| e.to_string())?;
        if let Err(error) = plugin.open_editor(WindowHandle::from_x11(window.window.resource_id()))
        {
            plugin
                .close_editor()
                .unwrap_or_else(|e| fatal(&format!("detaching failed Open: {e}")));
            return Err(error.to_string());
        }
        let result = (|| {
            window.resizable = plugin.editor_can_resize();
            let (width, height) = plugin
                .take_editor_resize_request()
                .map(Ok)
                .unwrap_or_else(|| plugin.get_editor_size())
                .map_err(|e| e.to_string())?;
            let size = Size::checked(width, height)?;
            window.set_size(size)?;
            window.discover_embedded()?;
            window.focus()?;
            Ok::<_, String>(size)
        })();
        match result {
            Ok(size) => {
                self.state.open = true;
                self.state.width = size.width;
                self.state.height = size.height;
                self.state.generation = self.state.generation.saturating_add(1);
                self.window = Some(window);
                Ok(())
            }
            Err(error) => {
                plugin
                    .close_editor()
                    .unwrap_or_else(|e| fatal(&format!("detaching failed Open: {e}")));
                Err(error)
            }
        }
    }
    fn close(&mut self) -> Result<(), String> {
        if let Some(mut window) = self.window.take() {
            let mut guard = self
                .plugin
                .lock()
                .unwrap_or_else(|_| fatal("plugin lock poisoned before editor detachment"));
            if let Some(plugin) = guard.as_mut() {
                plugin
                    .close_editor()
                    .unwrap_or_else(|e| fatal(&format!("detaching editor: {e}")));
            }
            window
                .destroy()
                .unwrap_or_else(|e| fatal(&format!("destroying editor: {e}")));
        }
        self.state.open = false;
        self.state.width = 0;
        self.state.height = 0;
        Ok(())
    }
    fn service(&mut self) {
        if let Some(window) = self.window.as_mut() {
            window
                .poll()
                .unwrap_or_else(|e| fatal(&format!("display connection/event failure: {e}")));
            // An externally destroyed parent is not safe for another plug-in callback.
            if window.signals.destroyed {
                fatal("native container was destroyed before plugin detach");
            }
            if window.signals.close {
                self.close().unwrap_or_else(|e| fatal(&e));
            }
        }
        let mut guard = self
            .plugin
            .lock()
            .unwrap_or_else(|_| fatal("plugin lock poisoned"));
        let Some(plugin) = guard.as_mut() else {
            return;
        };
        // Factory registrations can predate and outlive a view. Service while loaded,
        // including stopped transport and closed editor, on this same main thread.
        plugin.service_run_loop();
        let Some(window) = self.window.as_mut() else {
            return;
        };
        if std::mem::take(&mut window.signals.scale_changed)
            && let Err(error) = plugin.set_editor_scale_factor(window.scale())
        {
            eprintln!("Editor scale: {error}");
        }
        let size = if let Some((width, height)) = plugin.take_editor_resize_request() {
            window.signals.size = None; // Plugin request supersedes any old WM configure.
            // resizeView already called onSize synchronously; do not call it twice.
            Some(Size::checked(width, height).unwrap_or_else(|e| fatal(&e)))
        } else if let Some(size) = window.signals.size.take().filter(|s| *s != window.size) {
            if window.resizable {
                match plugin.resize_editor(size.width, size.height) {
                    Ok((width, height)) => {
                        Some(Size::checked(width, height).unwrap_or_else(|e| fatal(&e)))
                    }
                    Err(error) => {
                        eprintln!("Editor resize rejected: {error}");
                        Some(window.size)
                    }
                }
            } else {
                Some(window.size)
            }
        } else {
            None
        };
        if let Some(size) = size {
            window.set_size(size).unwrap_or_else(|e| fatal(&e));
            self.state.width = size.width;
            self.state.height = size.height;
        }
    }
    fn command(&mut self, command: IsolatedEditorCommand) -> HostResponse {
        let result = match command {
            IsolatedEditorCommand::Query => Ok(()),
            IsolatedEditorCommand::Open { owner } => self.open(owner),
            IsolatedEditorCommand::Focus => self
                .window
                .as_ref()
                .ok_or_else(|| "Editor is closed".to_string())
                .and_then(NativeWindow::focus),
            IsolatedEditorCommand::Close => self.close(),
        };
        match result {
            Ok(()) => HostResponse::EditorState { state: self.state },
            Err(message) => HostResponse::Error { message },
        }
    }
}

enum Input {
    Command(Box<HostCommand>),
    Invalid(String),
    Eof,
}
#[derive(Debug, PartialEq, Eq)]
enum CommandLine {
    Line(Vec<u8>),
    Oversized,
    Eof,
}
fn read_command_line(reader: &mut impl BufRead, limit: usize) -> io::Result<CommandLine> {
    let mut line = Vec::new();
    let mut oversized = false;
    loop {
        let bytes = reader.fill_buf()?;
        if bytes.is_empty() {
            return Ok(if oversized {
                CommandLine::Oversized
            } else if line.is_empty() {
                CommandLine::Eof
            } else {
                CommandLine::Line(line)
            });
        }
        let newline = bytes.iter().position(|&byte| byte == b'\n');
        let count = newline.unwrap_or(bytes.len());
        if !oversized {
            if line.len().saturating_add(count) <= limit {
                line.extend_from_slice(&bytes[..count]);
            } else {
                oversized = true;
                line.clear();
            }
        }
        reader.consume(count + usize::from(newline.is_some()));
        if newline.is_some() {
            return Ok(if oversized {
                CommandLine::Oversized
            } else {
                CommandLine::Line(line)
            });
        }
    }
}
fn read_stdin(tx: SyncSender<Input>) {
    let stdin = io::stdin();
    let mut reader = stdin.lock();
    loop {
        let input = match read_command_line(&mut reader, MAX_COMMAND_BYTES) {
            Ok(CommandLine::Line(line)) if line.iter().all(u8::is_ascii_whitespace) => continue,
            Ok(CommandLine::Line(line)) => match serde_json::from_slice(&line) {
                Ok(command) => Input::Command(Box::new(command)),
                Err(e) => Input::Invalid(format!("Invalid command: {e}")),
            },
            Ok(CommandLine::Oversized) => {
                Input::Invalid("Command exceeds bounded wire line limit".into())
            }
            Ok(CommandLine::Eof) => break,
            Err(error) => {
                let _ = tx.send(Input::Invalid(format!("Failed to read stdin: {error}")));
                break;
            }
        };
        let shutdown = matches!(&input, Input::Command(c) if matches!(**c, HostCommand::Shutdown));
        if tx.send(input).is_err() || shutdown {
            return;
        }
    }
    let _ = tx.send(Input::Eof);
}
fn handle_input(
    input: Input,
    editor: &mut Editor,
    protocol: &mut ProtocolChannel,
    sample_rate: &mut f64,
) -> bool {
    let command = match input {
        Input::Command(c) => *c,
        Input::Invalid(message) => {
            respond(protocol, &HostResponse::Error { message });
            return true;
        }
        Input::Eof => return false,
    };
    if matches!(command, HostCommand::Shutdown) {
        return false;
    }
    let response = match command {
        HostCommand::Editor { command } => editor.command(command),
        HostCommand::CreateGui => match editor.open(None) {
            Ok(()) => HostResponse::GuiCreated {
                width: editor.state.width,
                height: editor.state.height,
            },
            Err(message) => HostResponse::Error { message },
        },
        HostCommand::CloseGui => match editor.close() {
            Ok(()) => HostResponse::Success {
                message: "editor closed".into(),
            },
            Err(message) => HostResponse::Error { message },
        },
        command => {
            let changes_plugin = matches!(
                command,
                HostCommand::LoadPlugin { .. } | HostCommand::UnloadPlugin
            );
            if requires_editor_detach(&command)
                && let Err(error) = editor.close()
            {
                respond(
                    protocol,
                    &err(
                        "Failed to close editor before plugin state operation",
                        error,
                    ),
                );
                return true;
            }
            let response = handle(command, &editor.plugin, sample_rate, None);
            if changes_plugin {
                editor.plugin_changed();
            }
            response
        }
    };
    respond(protocol, &response);
    true
}

pub(super) fn run(plugin: SharedPlugin, mut protocol: ProtocolChannel) {
    let mut editor = Editor::new(plugin.clone());
    let (tx, rx) = mpsc::sync_channel(COMMAND_QUEUE_CAPACITY);
    if let Err(error) = std::thread::Builder::new()
        .name("vst3-helper-stdin".into())
        .spawn(move || read_stdin(tx))
    {
        respond(&mut protocol, &err("Failed to start stdin reader", error));
        return;
    }
    let mut sample_rate = 44_100.0;
    let mut running = true;
    while running {
        editor.service();
        let mut activity = false;
        for _ in 0..COMMAND_BATCH {
            match rx.try_recv() {
                Ok(input) => {
                    activity = true;
                    running = handle_input(input, &mut editor, &mut protocol, &mut sample_rate);
                    editor.service();
                    if !running {
                        break;
                    }
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    running = false;
                    break;
                }
            }
        }
        // Bounded low-latency wait wakes immediately for commands, without busy-spinning
        // when closed. Timers/X11 are serviced at least every 4 ms if plugin calls return.
        if running && !activity {
            match rx.recv_timeout(Duration::from_millis(4)) {
                Ok(input) => {
                    running = handle_input(input, &mut editor, &mut protocol, &mut sample_rate);
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => running = false,
            }
        }
    }
    editor.close().unwrap_or_else(|e| fatal(&e));
    if let Ok(mut guard) = plugin.lock() {
        *guard = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufReader, Cursor};
    #[test]
    fn linux_owner_is_explicitly_standalone() {
        assert!(validate_owner(None).is_ok());
        assert!(
            validate_owner(Some(IsolatedEditorOwner {
                window: 1,
                process_id: 1
            }))
            .is_err()
        );
    }
    #[test]
    fn dimensions_reject_invalid_and_overflow() {
        for (w, h) in [(0, 1), (-1, 1), (1, 0), (16_385, 2), (i32::MAX, 20)] {
            assert!(Size::checked(w, h).is_err());
        }
        assert!(Size::checked(16_384, 16_384).is_ok());
    }
    #[test]
    fn dpi_resources_are_bounded_and_no_double_scaling() {
        assert_eq!(resource_scale(b"Xft.dpi:\t144\n"), 1.5);
        for s in [
            b"Xft.dpi: NaN".as_slice(),
            b"Xft.dpi: inf",
            b"Xft.dpi: 0",
            b"Xft.dpi: 10000",
            b"",
        ] {
            assert_eq!(resource_scale(s), 1.0);
        }
    }
    #[test]
    fn malformed_input_is_drained_without_losing_next_command() {
        let mut reader = BufReader::with_capacity(2, Cursor::new(b"123456789\nok\nlast"));
        assert_eq!(
            read_command_line(&mut reader, 4).unwrap(),
            CommandLine::Oversized
        );
        assert_eq!(
            read_command_line(&mut reader, 4).unwrap(),
            CommandLine::Line(b"ok".to_vec())
        );
        assert_eq!(
            read_command_line(&mut reader, 4).unwrap(),
            CommandLine::Line(b"last".to_vec())
        );
        assert_eq!(read_command_line(&mut reader, 4).unwrap(), CommandLine::Eof);
    }
    #[test]
    fn state_operations_detach_but_feedback_does_not() {
        assert!(requires_editor_detach(&HostCommand::SaveState));
        assert!(requires_editor_detach(&HostCommand::UnloadPlugin));
        assert!(!requires_editor_detach(&HostCommand::NativeDirtyRevision));
    }
    #[test]
    fn closed_query_and_repeated_close_need_no_display() {
        let mut editor = Editor::new(std::sync::Arc::new(std::sync::Mutex::new(None)));
        for _ in 0..2 {
            assert!(
                matches!(editor.command(IsolatedEditorCommand::Close),HostResponse::EditorState { state } if state.supported && !state.open && state.generation==0)
            );
        }
        assert!(matches!(
            editor.command(IsolatedEditorCommand::Focus),
            HostResponse::Error { .. }
        ));
        assert!(matches!(
            editor.command(IsolatedEditorCommand::Open { owner: None }),
            HostResponse::Error { .. }
        ));
    }
}
