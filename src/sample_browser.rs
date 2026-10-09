//! Explicit, session-only local WAV browsing. No recursion, decoding, or audio work here.
//!
//! A single control-side worker inspects a bounded number of directory entries. Navigation
//! cancels obsolete work and coalesces to the latest request, rather than spawning more workers.

use std::{
    fs,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver},
    },
};

pub const MAX_FOLDER_ENTRIES: usize = 512;
pub const MAX_INSPECTED_ENTRIES: usize = 4_096;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum EntryKind {
    Folder,
    Wav,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SampleEntry {
    pub path: PathBuf,
    pub name: String,
    pub kind: EntryKind,
    pub bytes: Option<u64>,
}

impl SampleEntry {
    pub fn matches(&self, lowercase_query: &str) -> bool {
        self.name.to_lowercase().contains(lowercase_query)
    }
}

#[derive(Debug, Default)]
struct FolderListing {
    entries: Vec<SampleEntry>,
    limited: bool,
    skipped: usize,
}

fn is_wav_path(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            extension.eq_ignore_ascii_case("wav") || extension.eq_ignore_ascii_case("wave")
        })
}

fn scan_folder(
    path: &Path,
    canceled: &AtomicBool,
    max_entries: usize,
    max_inspected: usize,
) -> Result<FolderListing, String> {
    let mut listing = FolderListing::default();
    if canceled.load(Ordering::Relaxed) {
        return Ok(listing);
    }
    let directory = fs::read_dir(path).map_err(|error| format!("Cannot read folder: {error}"))?;
    for (inspected, item) in directory.enumerate() {
        if canceled.load(Ordering::Relaxed) {
            return Ok(FolderListing::default());
        }
        if inspected >= max_inspected || listing.entries.len() >= max_entries {
            listing.limited = true;
            break;
        }
        let item = match item {
            Ok(item) => item,
            Err(_) => {
                listing.skipped += 1;
                continue;
            }
        };
        let file_type = match item.file_type() {
            Ok(file_type) => file_type,
            Err(_) => {
                listing.skipped += 1;
                continue;
            }
        };
        // Never follow discovered symlinks, devices, sockets or FIFOs. An explicitly chosen
        // root can itself be a link; this policy is navigation behavior, not a security sandbox.
        if file_type.is_symlink() {
            listing.skipped += 1;
            continue;
        }
        let path = item.path();
        let kind = if file_type.is_dir() {
            EntryKind::Folder
        } else if file_type.is_file() && is_wav_path(&path) {
            EntryKind::Wav
        } else {
            continue;
        };
        let bytes = if kind == EntryKind::Wav {
            match item.metadata() {
                Ok(metadata) if metadata.is_file() => Some(metadata.len()),
                _ => {
                    listing.skipped += 1;
                    continue;
                }
            }
        } else {
            None
        };
        listing.entries.push(SampleEntry {
            name: item.file_name().to_string_lossy().into_owned(),
            path,
            kind,
            bytes,
        });
    }
    listing
        .entries
        .sort_by_cached_key(|entry| (entry.kind, entry.name.to_lowercase(), entry.path.clone()));
    Ok(listing)
}

struct ScanResult {
    generation: u64,
    path: PathBuf,
    listing: Result<FolderListing, String>,
}

struct ActiveScan {
    canceled: Arc<AtomicBool>,
    receiver: Receiver<ScanResult>,
}

#[derive(Default)]
pub struct SampleBrowser {
    pub directory: Option<PathBuf>,
    pub entries: Vec<SampleEntry>,
    pub selected: Option<PathBuf>,
    pub notice: Option<String>,
    pub limited: bool,
    generation: u64,
    desired: Option<(u64, PathBuf)>,
    active: Option<ActiveScan>,
}

impl SampleBrowser {
    pub fn busy(&self) -> bool {
        self.active.is_some() || self.desired.is_some()
    }

    pub fn selected_entry(&self, lowercase_query: &str) -> Option<&SampleEntry> {
        let selected = self.selected.as_ref()?;
        self.entries.iter().find(|entry| {
            entry.kind == EntryKind::Wav
                && &entry.path == selected
                && entry.matches(lowercase_query)
        })
    }

    pub fn navigate(&mut self, path: PathBuf) {
        self.invalidate();
        self.directory = Some(path.clone());
        self.entries.clear();
        self.selected = None;
        self.notice = None;
        self.limited = false;
        self.desired = Some((self.generation, path));
        self.start_desired();
    }

    pub fn cancel(&mut self) {
        self.invalidate();
        self.notice = Some("Folder scan canceled. Refresh to try again.".into());
    }

    fn invalidate(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.desired = None;
        if let Some(active) = &self.active {
            active.canceled.store(true, Ordering::Relaxed);
        }
    }

    fn start_desired(&mut self) {
        if self.active.is_some() {
            return;
        }
        let Some((generation, path)) = self.desired.take() else {
            return;
        };
        let canceled = Arc::new(AtomicBool::new(false));
        let worker_cancel = Arc::clone(&canceled);
        let (sender, receiver) = mpsc::sync_channel(1);
        match std::thread::Builder::new()
            .name("sample-folder-list".into())
            .spawn(move || {
                let listing = scan_folder(
                    &path,
                    &worker_cancel,
                    MAX_FOLDER_ENTRIES,
                    MAX_INSPECTED_ENTRIES,
                );
                // One result, one bounded slot; the UI never joins this worker or waits on I/O.
                let _ = sender.try_send(ScanResult {
                    generation,
                    path,
                    listing,
                });
            }) {
            Ok(_) => self.active = Some(ActiveScan { canceled, receiver }),
            Err(error) => self.notice = Some(format!("Cannot start folder scan: {error}")),
        }
    }

    pub fn poll(&mut self) {
        let Some(active) = &self.active else {
            self.start_desired();
            return;
        };
        match active.receiver.try_recv() {
            Ok(result) => {
                self.active = None;
                self.accept(result);
                self.start_desired();
            }
            Err(mpsc::TryRecvError::Disconnected) => {
                let canceled = active.canceled.load(Ordering::Relaxed);
                self.active = None;
                if !canceled {
                    self.notice =
                        Some("Folder scan stopped unexpectedly. Refresh to retry.".into());
                }
                self.start_desired();
            }
            Err(mpsc::TryRecvError::Empty) => {}
        }
    }

    fn accept(&mut self, result: ScanResult) {
        if result.generation != self.generation || self.directory.as_ref() != Some(&result.path) {
            return;
        }
        match result.listing {
            Ok(listing) => {
                self.entries = listing.entries;
                self.limited = listing.limited;
                self.notice = (listing.skipped != 0).then(|| {
                    format!(
                        "Skipped {} unreadable or symbolic-link entries.",
                        listing.skipped
                    )
                });
            }
            Err(error) => self.notice = Some(error),
        }
    }
}

impl Drop for ActiveScan {
    fn drop(&mut self) {
        self.canceled.store(true, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        sync::atomic::AtomicU64,
        time::{Duration, Instant},
    };

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "citrus-browser-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
        fn file(&self, name: &str) -> PathBuf {
            let path = self.0.join(name);
            fs::write(&path, b"format is checked only when explicitly imported").unwrap();
            path
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }
    fn listing(path: &Path) -> FolderListing {
        scan_folder(
            path,
            &AtomicBool::new(false),
            MAX_FOLDER_ENTRIES,
            MAX_INSPECTED_ENTRIES,
        )
        .unwrap()
    }
    fn finish(browser: &mut SampleBrowser) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while browser.busy() {
            assert!(Instant::now() < deadline, "folder worker did not finish");
            browser.poll();
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    #[test]
    fn idle_browser_does_no_automatic_scan() {
        let mut browser = SampleBrowser::default();
        browser.poll();
        assert!(!browser.busy());
        assert!(browser.directory.is_none());
        assert!(browser.entries.is_empty());
    }

    #[test]
    fn lists_only_direct_folders_and_wav_candidates_sorted_folder_first() {
        let fixture = Fixture::new();
        fixture.file("Z.wav");
        fixture.file("a.WAVE");
        fixture.file("b.WaV");
        fixture.file("ignored.mp3");
        fixture.file("ignored.aiff");
        fixture.file("ignored.wav.txt");
        let child = fixture.0.join("samples");
        fs::create_dir(&child).unwrap();
        fs::write(child.join("nested.wav"), b"unread").unwrap();
        let result = listing(&fixture.0);
        assert_eq!(
            result
                .entries
                .iter()
                .map(|entry| entry.name.as_str())
                .collect::<Vec<_>>(),
            ["samples", "a.WAVE", "b.WaV", "Z.wav"]
        );
        assert_eq!(result.entries[0].kind, EntryKind::Folder);
        assert!(result.entries[1].bytes.is_some());
        assert!(!result.limited);
    }

    #[test]
    fn file_limit_is_visible_and_memory_bounded() {
        let fixture = Fixture::new();
        for index in 0..6 {
            fixture.file(&format!("{index}.wav"));
        }
        let result = scan_folder(&fixture.0, &AtomicBool::new(false), 2, 99).unwrap();
        assert_eq!(result.entries.len(), 2);
        assert!(result.limited);
    }

    #[test]
    fn inspection_limit_includes_unsupported_files() {
        let fixture = Fixture::new();
        for index in 0..6 {
            fixture.file(&format!("{index}.mp3"));
        }
        let result = scan_folder(&fixture.0, &AtomicBool::new(false), 99, 2).unwrap();
        assert!(result.entries.is_empty());
        assert!(result.limited);
    }

    #[test]
    fn missing_folder_and_regular_file_report_error_without_old_rows() {
        let fixture = Fixture::new();
        let file = fixture.file("a.wav");
        let mut browser = SampleBrowser::default();
        browser.navigate(fixture.0.clone());
        finish(&mut browser);
        browser.selected = Some(file.clone());
        browser.navigate(file);
        finish(&mut browser);
        assert!(browser.entries.is_empty());
        assert!(browser.selected.is_none());
        assert!(
            browser
                .notice
                .as_ref()
                .unwrap()
                .contains("Cannot read folder")
        );
        browser.navigate(fixture.0.join("missing"));
        finish(&mut browser);
        assert!(
            browser
                .notice
                .as_ref()
                .unwrap()
                .contains("Cannot read folder")
        );
    }

    #[test]
    fn canceled_scan_does_not_touch_missing_directory() {
        let result = scan_folder(
            Path::new("/not-a-citrus-test-folder"),
            &AtomicBool::new(true),
            2,
            2,
        )
        .unwrap();
        assert!(result.entries.is_empty());
    }

    #[test]
    fn cancel_rejects_finished_but_unconsumed_result() {
        let fixture = Fixture::new();
        fixture.file("a.wav");
        let mut browser = SampleBrowser::default();
        browser.navigate(fixture.0.clone());
        browser.cancel();
        finish(&mut browser);
        assert!(browser.entries.is_empty());
        assert!(browser.notice.as_ref().unwrap().contains("canceled"));
    }

    #[test]
    fn latest_navigation_coalesces_while_previous_worker_is_active() {
        let fixture = Fixture::new();
        let newest = fixture.0.join("newest");
        fs::create_dir(&newest).unwrap();
        fs::write(newest.join("winner.wav"), b"data").unwrap();
        let (sender, receiver) = mpsc::sync_channel(1);
        let canceled = Arc::new(AtomicBool::new(false));
        let mut browser = SampleBrowser {
            active: Some(ActiveScan {
                canceled: Arc::clone(&canceled),
                receiver,
            }),
            ..Default::default()
        };
        browser.navigate(fixture.0.clone());
        browser.navigate(fixture.0.join("obsolete missing"));
        browser.navigate(newest.clone());
        assert!(canceled.load(Ordering::Relaxed));
        assert_eq!(browser.desired.as_ref().unwrap().1, newest);
        sender
            .send(ScanResult {
                generation: 0,
                path: fixture.0.clone(),
                listing: Err("old error".into()),
            })
            .unwrap();
        finish(&mut browser);
        assert_eq!(browser.entries.len(), 1);
        assert_eq!(browser.entries[0].name, "winner.wav");
        assert!(browser.notice.is_none());
    }

    #[test]
    fn stale_same_folder_refresh_result_cannot_replace_new_state() {
        let fixture = Fixture::new();
        let mut browser = SampleBrowser {
            directory: Some(fixture.0.clone()),
            generation: 2,
            ..Default::default()
        };
        browser.accept(ScanResult {
            generation: 1,
            path: fixture.0.clone(),
            listing: Err("stale".into()),
        });
        assert!(browser.notice.is_none());
    }

    #[test]
    fn hidden_or_folder_selection_cannot_be_imported_and_refresh_clears_selection() {
        let fixture = Fixture::new();
        let file = fixture.file("Kick.WAV");
        let folder = fixture.0.join("child");
        fs::create_dir(&folder).unwrap();
        let mut browser = SampleBrowser::default();
        browser.navigate(fixture.0.clone());
        finish(&mut browser);
        browser.selected = Some(file);
        assert!(browser.selected_entry("kick").is_some());
        assert!(browser.selected_entry("snare").is_none());
        browser.selected = Some(folder);
        assert!(browser.selected_entry("").is_none());
        browser.navigate(fixture.0.clone());
        assert!(browser.selected.is_none());
        finish(&mut browser);
    }

    #[test]
    fn dropping_browser_requests_cancellation_without_waiting() {
        let (_sender, receiver) = mpsc::sync_channel(1);
        let canceled = Arc::new(AtomicBool::new(false));
        let browser = SampleBrowser {
            active: Some(ActiveScan {
                canceled: Arc::clone(&canceled),
                receiver,
            }),
            ..Default::default()
        };
        drop(browser);
        assert!(canceled.load(Ordering::Relaxed));
    }

    #[cfg(unix)]
    #[test]
    fn discovered_symlinks_are_skipped_including_folder_cycles() {
        use std::os::unix::fs::symlink;
        let fixture = Fixture::new();
        let file = fixture.file("original.wav");
        symlink(file, fixture.0.join("linked.wav")).unwrap();
        symlink(&fixture.0, fixture.0.join("cycle")).unwrap();
        let result = listing(&fixture.0);
        assert_eq!(result.entries.len(), 1);
        assert_eq!(result.skipped, 2);
    }

    #[cfg(unix)]
    #[test]
    fn non_utf8_names_keep_exact_filesystem_identity() {
        use std::{ffi::OsStr, os::unix::ffi::OsStrExt};
        let fixture = Fixture::new();
        let path = fixture.0.join(OsStr::from_bytes(b"sample\xff.wav"));
        fs::write(&path, b"data").unwrap();
        let result = listing(&fixture.0);
        assert_eq!(result.entries[0].path, path);
        assert!(result.entries[0].name.contains('\u{fffd}'));
    }
}
