//! Plugin-free admission, driver accounting and exact playback oracle.
//! No SDK import, plug-in factory or callback method is reachable from this file.
use super::oracles::sha256_hex;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

pub const SELECTOR: &str =
    "app::clip_render_job::genuine::genuine_selected_clip_app_conversion_undo_redo";
pub const PLAN_SHA256: &str = "bb3fbc835dfad5d4275f4b21772e3c452e0d42cee9c7b69ae0f79ea28978a68d";
pub const BASE_COMMIT: &str = "4a56f3755cc8216da7da69ed07e4b7c1d20d6aae";
pub const SOURCE_ROOT: &str = "/workspace/shared/DAW-selected-clip-app-genuine";
pub const PLUGIN: &str =
    "/workspace/shared/daw-plugin-fixtures-20261010/extracted/surge/Surge XT.vst3";
pub const CLASS: &str = "ABCDEF019182FAEB566D624153675854";
pub const MAX_PUMPS: usize = 4096;
pub const MAX_POLLS: usize = 12_000;
pub const CLEANUP_PUMPS: usize = 116;
pub const A: u64 = 30_000;
pub const RETAINED: usize = 36_000;
pub const PLAY_SEGMENTS: [usize; 3] = [300, 704, 704];
pub const CONTROL_CEILINGS: [usize; 8] = [640, 128, 512, 128, 384, 128, 128, 128];
pub const STAGES: [&str; 10] = [
    "preflight",
    "source_load",
    "source_audibility_stop_base",
    "conversion",
    "raw_audit",
    "converted_playback_stop",
    "undo",
    "redo",
    "redo_playback_stop",
    "cleanup",
];

#[derive(Clone, Debug, Deserialize)]
pub struct FilePin {
    pub path: PathBuf,
    pub sha256: String,
}
#[derive(Clone, Debug)]
pub struct RunBinding {
    pub raw: serde_json::Value,
    pub run: PathBuf,
    pub output_directory: PathBuf,
    pub binding_hash: String,
    pins: Vec<FilePin>,
}
fn must(ok: bool, reason: &str) -> Result<(), String> {
    if ok { Ok(()) } else { Err(reason.into()) }
}
fn bounded_read(path: &Path, ceiling: u64) -> Result<Vec<u8>, String> {
    let m = fs::symlink_metadata(path).map_err(|e| format!("{}: {e}", path.display()))?;
    must(
        m.file_type().is_file() && m.len() <= ceiling,
        "pin must be a bounded regular non-symlink file",
    )?;
    use std::io::Read;
    let mut bytes = Vec::new();
    fs::File::open(path)
        .map_err(|e| e.to_string())?
        .take(ceiling + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    must(
        bytes.len() as u64 <= ceiling,
        "pinned file grew beyond bounded read",
    )?;
    Ok(bytes)
}
impl FilePin {
    pub fn verify(&self) -> Result<(), String> {
        must(
            self.path.is_absolute() && is_hash(&self.sha256),
            "pin path/hash invalid",
        )?;
        let bytes = bounded_read(&self.path, 1024 * 1024 * 1024)?;
        must(
            sha256_hex(&bytes) == self.sha256,
            &format!("pin changed: {}", self.path.display()),
        )
    }
}
pub fn is_hash(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|v| v.is_ascii_digit() || (b'a'..=b'f').contains(&v))
}
pub fn validate_explicit_opt_in(
    opt: Option<&str>,
    hash: Option<&str>,
    args: &[String],
) -> Result<String, String> {
    let hash = hash.ok_or("explicit binding hash missing")?;
    must(is_hash(hash), "binding hash malformed")?;
    must(
        opt == Some(format!("selected-clip-app:{hash}").as_str()),
        "exact run-bound opt-in missing",
    )?;
    let expected = [
        SELECTOR,
        "--ignored",
        "--exact",
        "--test-threads=1",
        "--nocapture",
    ];
    must(
        args.len() == expected.len() && args.iter().zip(expected).all(|(a, b)| a == b),
        "only the exact ignored selector and frozen arguments are admitted",
    )?;
    Ok(hash.into())
}
impl RunBinding {
    pub fn preflight() -> Result<(Self, [u8; 32]), String> {
        let args = std::env::args().skip(1).collect::<Vec<_>>();
        let opt = std::env::var("CITRUS_APP_GENUINE_OPT_IN").ok();
        let hash = std::env::var("CITRUS_APP_GENUINE_BINDING_SHA256").ok();
        let hash = validate_explicit_opt_in(opt.as_deref(), hash.as_deref(), &args)?;
        let path = PathBuf::from(
            std::env::var("CITRUS_APP_GENUINE_BINDING").map_err(|_| "binding path missing")?,
        );
        must(path.is_absolute(), "binding path must be absolute")?;
        let bytes = bounded_read(&path, 4 * 1024 * 1024)?;
        must(sha256_hex(&bytes) == hash, "run binding hash mismatch")?;
        let raw: serde_json::Value = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
        must(
            raw["schema"] == 1
                && raw["qualification"] == "genuine-selected-clip-app"
                && raw["selector"] == SELECTOR
                && raw["source_root"] == SOURCE_ROOT
                && raw["base_commit"] == BASE_COMMIT
                && raw["source_commit"]
                    .as_str()
                    .is_some_and(|s| s.len() == 40 && s.bytes().all(|b| b.is_ascii_hexdigit()))
                && raw["proposal"]["sha256"] == PLAN_SHA256,
            "immutable binding identity mismatch",
        )?;
        let run = PathBuf::from(raw["run_dir"].as_str().ok_or("run directory missing")?);
        let output_directory = PathBuf::from(
            raw["output_directory"]
                .as_str()
                .ok_or("output directory missing")?,
        );
        must(
            run.is_absolute()
                && output_directory == run.join("private/output")
                && path == run.join("run-binding.json"),
            "run-owned path binding mismatch",
        )?;
        let current = std::env::current_exe().map_err(|e| e.to_string())?;
        must(
            current == run.join("bin/genuine-app-test"),
            "copied exact test executable mismatch",
        )?;
        let mut command = vec![current.to_string_lossy().into_owned()];
        command.extend(args);
        must(
            raw["command"]["argv"] == serde_json::json!(command)
                && raw["command"]["cwd"]
                    == std::env::current_dir()
                        .map_err(|e| e.to_string())?
                        .to_string_lossy()
                        .as_ref(),
            "command/cwd differs from run binding",
        )?;
        let mut pins = Vec::new();
        for key in [
            "proposal",
            "source_manifest",
            "harness_executable",
            "ordinary_helper",
            "build_binding",
            "python_interpreter",
        ] {
            pins.push(
                serde_json::from_value::<FilePin>(raw[key].clone())
                    .map_err(|e| format!("{key}: {e}"))?,
            );
        }
        for key in ["harness_sources", "reviewed_evidence", "asset_pins"] {
            let entries = raw[key]
                .as_array()
                .ok_or_else(|| format!("{key} inventory missing"))?;
            must(
                !entries.is_empty() && entries.len() <= 4096,
                "bounded nonempty pin inventory required",
            )?;
            for item in entries {
                pins.push(
                    serde_json::from_value::<FilePin>(item.clone()).map_err(|e| e.to_string())?,
                );
            }
        }
        let manifest_pin: FilePin =
            serde_json::from_value(raw["source_manifest"].clone()).map_err(|e| e.to_string())?;
        manifest_pin.verify()?;
        let manifest: serde_json::Value =
            serde_json::from_slice(&bounded_read(&manifest_pin.path, 4 * 1024 * 1024)?)
                .map_err(|e| e.to_string())?;
        let files = manifest["files"]
            .as_object()
            .ok_or("source manifest files mapping")?;
        must(
            !files.is_empty() && files.len() <= 4096,
            "source manifest bounds",
        )?;
        let mut seen = BTreeSet::new();
        for (name, digest) in files {
            let relative = Path::new(name);
            must(
                !relative.is_absolute()
                    && relative
                        .components()
                        .all(|c| matches!(c, std::path::Component::Normal(_))),
                "source manifest path traversal",
            )?;
            seen.insert(name.as_str());
            pins.push(FilePin {
                path: Path::new(SOURCE_ROOT).join(relative),
                sha256: digest.as_str().ok_or("manifest hash")?.into(),
            });
        }
        for expected in [
            "src/app/clip_render_genuine.rs",
            "src/app/clip_render_genuine_support.rs",
            "src/app/clip_render_genuine_oracles.rs",
            "src/app/clip_render_genuine_phase_fixture.json",
            "src/app/clip_render_job.rs",
            "src/audio.rs",
            "src/plugin_runtime.rs",
            "src/instrument_bounce_strict_backend.rs",
            "Cargo.lock",
        ] {
            must(seen.contains(expected), "required source pin absent")?;
        }
        let expected_assets = [
            (
                "/workspace/shared/daw-plugin-fixtures-20261010/downloads/surge-xt-linux-1.3.4-pluginsonly.tar.gz",
                "dd431b75f5fa197c4bffa6ca27ca46970f0a94c834119bb1db7decdeec4c28db",
            ),
            (
                "/workspace/shared/daw-plugin-fixtures-20261010/extracted/surge/Surge XT.vst3/Contents/x86_64-linux/Surge XT.so",
                "b8584398f314819241ca7287117d5559ed838d37ef08faab9847f27ba2993cec",
            ),
            (
                "/workspace/shared/daw-plugin-fixtures-20261010/fixture-manifest.json",
                "ebc752385fac2068fcbdd38bb63278fe68be2f125e588c45d9ec161fbc3c6375",
            ),
        ];
        let assets = raw["asset_pins"].as_array().ok_or("asset pins absent")?;
        must(
            assets.len() == 3
                && assets
                    .iter()
                    .zip(expected_assets)
                    .all(|(pin, (path, hash))| pin["path"] == path && pin["sha256"] == hash),
            "exact sorted three-file official asset set differs",
        )?;
        for key in [
            "HOME",
            "DISPLAY",
            "WAYLAND_DISPLAY",
            "XDG_RUNTIME_DIR",
            "PATH",
            "LD_LIBRARY_PATH",
            "LD_PRELOAD",
            "LD_AUDIT",
            "LD_DEBUG",
            "VST3_PATH",
        ] {
            let actual = std::env::var(key).ok();
            must(
                raw["environment"][key] == serde_json::json!(actual),
                "inherited environment differs from binding",
            )?;
        }
        pins.push(FilePin {
            path: current,
            sha256: raw["harness_executable"]["sha256"]
                .as_str()
                .ok_or("test executable hash")?
                .into(),
        });
        pins.push(FilePin {
            path: run.join("private/vst3-host-helper.real"),
            sha256: raw["ordinary_helper"]["sha256"]
                .as_str()
                .ok_or("helper hash")?
                .into(),
        });
        let launcher_hash = raw["harness_sources"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| {
                p["path"]
                    .as_str()
                    .is_some_and(|s| s.ends_with("/helper_launcher.py.in"))
            })
            .and_then(|p| p["sha256"].as_str())
            .ok_or("launcher source pin absent")?;
        pins.push(FilePin {
            path: run.join("bin/vst3-host-helper"),
            sha256: launcher_hash.into(),
        });
        let binding = Self {
            raw,
            run,
            output_directory,
            binding_hash: hash.clone(),
            pins,
        };
        binding.verify_pins()?;
        must(
            !binding.output_directory.exists(),
            "output directory already exists; no retry",
        )?;
        fs::create_dir(&binding.output_directory).map_err(|e| format!("create-new output: {e}"))?;
        let mut token = [0; 32];
        for (i, slot) in token.iter_mut().enumerate() {
            *slot = u8::from_str_radix(&hash[i * 2..i * 2 + 2], 16).map_err(|e| e.to_string())?;
        }
        Ok((binding, token))
    }
    pub fn verify_pins(&self) -> Result<(), String> {
        let mut verified = std::collections::BTreeMap::<PathBuf, String>::new();
        for pin in &self.pins {
            if let Some(hash) = verified.get(&pin.path) {
                must(hash == &pin.sha256, "conflicting repeated path pin")?;
                continue;
            }
            pin.verify()?;
            verified.insert(pin.path.clone(), pin.sha256.clone());
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
pub enum Charge {
    Control(usize),
    Playing(usize),
    Stopped(usize),
    FailureCleanup,
}
#[derive(Debug, Default, Serialize)]
pub struct Budget {
    pub controls: [usize; 8],
    pub playing: [usize; 3],
    pub stopped: [usize; 3],
    pub cleanup: usize,
    pub total: usize,
    pub polls: usize,
}
impl Budget {
    pub fn charge(&mut self, charge: Charge) -> Result<(), String> {
        must(self.total < MAX_PUMPS, "4096 total callback-pump ceiling")?;
        let (used, cap) = match charge {
            Charge::Control(i) => (
                self.controls.get_mut(i).ok_or("control stage index")?,
                *CONTROL_CEILINGS.get(i).ok_or("control ceiling index")?,
            ),
            Charge::Playing(i) => (
                self.playing.get_mut(i).ok_or("play pass index")?,
                *PLAY_SEGMENTS.get(i).ok_or("play ceiling index")?,
            ),
            Charge::Stopped(i) => (self.stopped.get_mut(i).ok_or("stop pass index")?, 32),
            Charge::FailureCleanup => (&mut self.cleanup, CLEANUP_PUMPS),
        };
        must(*used < cap, "nontransferable stage callback ceiling")?;
        *used += 1;
        self.total += 1;
        Ok(())
    }
    pub fn poll(&mut self) -> Result<(), String> {
        must(self.polls < MAX_POLLS, "bounded controller-poll ceiling")?;
        self.polls += 1;
        Ok(())
    }
    pub fn complete(&self) -> bool {
        self.playing == PLAY_SEGMENTS && self.stopped == [32; 3] && self.cleanup == 0
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cadence {
    Measured,
    Control,
    Poll,
}
impl Cadence {
    pub fn minimum(self) -> Duration {
        Duration::from_millis(match self {
            Self::Measured => 4,
            Self::Control => 20,
            Self::Poll => 10,
        })
    }
}
#[derive(Default)]
pub struct Pacer {
    last_pump: Option<Instant>,
    last_poll: Option<Instant>,
}
impl Pacer {
    pub fn wait(&mut self, cadence: Cadence, outer: Instant) -> Result<(), String> {
        let last = if cadence == Cadence::Poll {
            &mut self.last_poll
        } else {
            &mut self.last_pump
        };
        if let Some(prior) = *last {
            let next = prior + cadence.minimum();
            if let Some(delay) = next.checked_duration_since(Instant::now()) {
                must(next < outer, "cadence reaches outer deadline")?;
                std::thread::sleep(delay);
            }
        }
        must(
            Instant::now() < outer,
            "outer 120-second qualification deadline",
        )?;
        // Late service schedules from now; no catch-up burst or nominal-time replay.
        *last = Some(Instant::now());
        Ok(())
    }
}

/// A paused boolean alone is insufficient. Completed output and pre-roll ownership
/// are deliberately absent from this predicate: these are reported, never drained.
#[derive(Clone, Copy, Debug, Default)]
pub struct Quiescence {
    pub exact_paused_generation_epoch: bool,
    pub resync_or_fault: bool,
    pub partial_or_unsubmitted: bool,
    pub admitted_edit_or_safety: bool,
    pub input_consumed: bool,
    pub submitted: u64,
    pub completed: u64,
}
impl Quiescence {
    pub fn pure_wait(self) -> bool {
        self.exact_paused_generation_epoch
            && !self.resync_or_fault
            && !self.partial_or_unsubmitted
            && !self.admitted_edit_or_safety
            && self.input_consumed
            && self.submitted == self.completed
    }
}

pub fn signed_zero_equal(a: f32, b: f32) -> bool {
    a.to_bits() == b.to_bits() || (a == 0.0 && b == 0.0)
}
pub fn playback_sample(pcm: &[f32], frame: u64, channel: usize) -> Result<f32, String> {
    must(
        pcm.len() == RETAINED * 2 && channel < 2,
        "exact stereo retained oracle extent",
    )?;
    if !(A..A + RETAINED as u64).contains(&frame) {
        return Ok(0.0);
    }
    let i = (frame - A) as usize;
    let j = (i + 1).min(RETAINED - 1);
    let source = pcm[i * 2 + channel] + (pcm[j * 2 + channel] - pcm[i * 2 + channel]) * 0.0_f32;
    let gain = 0.75_f32 * 1.0_f32;
    let track = 0.0_f32 + source * gain;
    let track = track * 1.0_f32 * 1.0_f32;
    let routed = track * 1.0_f32;
    let master = 0.0_f32 + routed;
    let master = master * 1.0_f32 * 1.0_f32;
    Ok(master.tanh())
}
pub fn require_audible_windows(
    samples: &[f32],
    windows: &[(usize, usize)],
) -> Result<serde_json::Value, String> {
    must(
        samples.len().is_multiple_of(2) && samples.iter().all(|x| x.is_finite() && x.abs() <= 1.0),
        "nonfinite or non-unit-range output",
    )?;
    let mut metrics = Vec::new();
    for &(start, end) in windows {
        let slice = samples
            .get(start * 2..end * 2)
            .ok_or("fixed window absent")?;
        must(!slice.is_empty(), "empty fixed window")?;
        let peak = slice.iter().map(|x| f64::from(x.abs())).fold(0.0, f64::max);
        let rms =
            (slice.iter().map(|x| f64::from(*x).powi(2)).sum::<f64>() / slice.len() as f64).sqrt();
        must(peak > 1e-6 && rms > 1e-8, "fixed held window inaudible")?;
        metrics.push(serde_json::json!({"start":start,"end":end,"peak":peak,"rms":rms}));
    }
    Ok(serde_json::Value::Array(metrics))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn discovery_and_unbound_ignored_execution_refuse() {
        let args = vec![
            SELECTOR.into(),
            "--ignored".into(),
            "--exact".into(),
            "--test-threads=1".into(),
            "--nocapture".into(),
        ];
        assert!(validate_explicit_opt_in(None, None, &args).is_err());
        let h = "a".repeat(64);
        let opt = format!("selected-clip-app:{h}");
        assert!(validate_explicit_opt_in(Some(&opt), Some(&h), &args).is_ok());
        for bad in [
            vec!["--list".into()],
            vec!["--ignored".into()],
            vec![
                SELECTOR.into(),
                "--ignored".into(),
                "--exact".into(),
                "--test-threads=2".into(),
            ],
        ] {
            assert!(validate_explicit_opt_in(Some(&opt), Some(&h), &bad).is_err());
        }
    }
    #[test]
    fn stage_budgets_cannot_borrow_or_double_charge_activation() {
        let mut b = Budget::default();
        for _ in 0..CONTROL_CEILINGS[0] {
            b.charge(Charge::Control(0)).unwrap();
        }
        assert!(b.charge(Charge::Control(0)).is_err());
        assert_eq!(b.controls[1], 0);
        for (p, &segments) in PLAY_SEGMENTS.iter().enumerate() {
            for _ in 0..segments {
                b.charge(Charge::Playing(p)).unwrap();
            }
            assert!(b.charge(Charge::Playing(p)).is_err());
            for _ in 0..32 {
                b.charge(Charge::Stopped(p)).unwrap();
            }
            assert!(b.charge(Charge::Stopped(p)).is_err());
        }
        assert!(b.complete());
        assert_eq!(
            CONTROL_CEILINGS.iter().sum::<usize>()
                + PLAY_SEGMENTS.iter().sum::<usize>()
                + 96
                + CLEANUP_PUMPS,
            MAX_PUMPS
        );
    }
    #[test]
    fn fixed_cadence_never_has_catch_up_permission() {
        assert_eq!(Cadence::Measured.minimum(), Duration::from_millis(4));
        assert_eq!(Cadence::Control.minimum(), Duration::from_millis(20));
        assert_eq!(Cadence::Poll.minimum(), Duration::from_millis(10));
    }
    #[test]
    fn paused_control_service_requires_exact_drained_source() {
        let q = Quiescence {
            exact_paused_generation_epoch: true,
            input_consumed: true,
            submitted: 17,
            completed: 17,
            ..Default::default()
        };
        assert!(q.pure_wait());
        for q in [
            Quiescence {
                exact_paused_generation_epoch: false,
                ..q
            },
            Quiescence {
                resync_or_fault: true,
                ..q
            },
            Quiescence {
                partial_or_unsubmitted: true,
                ..q
            },
            Quiescence {
                admitted_edit_or_safety: true,
                ..q
            },
            Quiescence {
                input_consumed: false,
                ..q
            },
            Quiescence { completed: 16, ..q },
        ] {
            assert!(!q.pure_wait());
        }
    }
    #[test]
    fn nonzero_origin_exact_oracle_and_signed_zero_only() {
        let mut p = vec![0.25; RETAINED * 2];
        p[1] = -0.25;
        assert_eq!(playback_sample(&p, A - 1, 0).unwrap(), 0.0);
        assert_eq!(playback_sample(&p, A + RETAINED as u64, 0).unwrap(), 0.0);
        assert_eq!(
            playback_sample(&p, A, 0).unwrap().to_bits(),
            (0.25_f32 * 0.75).tanh().to_bits()
        );
        assert_eq!(
            playback_sample(&p, A, 1).unwrap().to_bits(),
            (-0.25_f32 * 0.75).tanh().to_bits()
        );
        assert!(signed_zero_equal(0.0, -0.0));
        assert!(!signed_zero_equal(
            1.0,
            f32::from_bits(1.0_f32.to_bits() - 1)
        ));
    }
}

/// Linux driver-thread read-only CLOCK_MONOTONIC. Never reachable from a callback,
/// and never changes a clock, timer, signal handler, SDK state or environment.
#[repr(C)]
struct LinuxTimespec {
    seconds: std::ffi::c_long,
    nanoseconds: std::ffi::c_long,
}
unsafe extern "C" {
    fn clock_gettime(clock: std::ffi::c_int, value: *mut LinuxTimespec) -> std::ffi::c_int;
}
pub fn monotonic_ns() -> Result<u64, String> {
    let mut value = LinuxTimespec {
        seconds: 0,
        nanoseconds: 0,
    };
    // CLOCK_MONOTONIC is Linux clock id1. The writable pointer is valid for the
    // complete call; return values are validated before arithmetic or use.
    let result = unsafe { clock_gettime(1, &mut value) };
    must(
        result == 0,
        "read-only clock_gettime(CLOCK_MONOTONIC) failed",
    )?;
    checked_monotonic_ns(value.seconds as i128, value.nanoseconds as i128)
}
fn checked_monotonic_ns(seconds: i128, nanoseconds: i128) -> Result<u64, String> {
    must(
        seconds >= 0 && (0..1_000_000_000).contains(&nanoseconds),
        "invalid monotonic timespec",
    )?;
    seconds
        .checked_mul(1_000_000_000)
        .and_then(|v| v.checked_add(nanoseconds))
        .and_then(|v| u64::try_from(v).ok())
        .ok_or_else(|| "monotonic timespec overflow".into())
}
#[cfg(test)]
mod clock_and_hash_tests {
    use super::*;
    #[test]
    fn monotonic_arithmetic_rejects_invalid_or_overflowed_timespec() {
        assert_eq!(checked_monotonic_ns(2, 3).unwrap(), 2_000_000_003);
        assert!(checked_monotonic_ns(-1, 0).is_err());
        assert!(checked_monotonic_ns(0, -1).is_err());
        assert!(checked_monotonic_ns(0, 1_000_000_000).is_err());
        assert!(checked_monotonic_ns(i128::MAX, 0).is_err());
    }
    #[test]
    fn representative_128mib_hashing_cost_diagnostic_plugin_free() {
        let bytes = vec![0u8; 128 * 1024 * 1024];
        let start = Instant::now();
        let hash = sha256_hex(&bytes);
        let elapsed = start.elapsed();
        assert!(is_hash(&hash));
        eprintln!(
            "genuine App preflight diagnostic: pure SHA256 128MiB took {elapsed:?}; this is not a plugin qualification or permission to change its120s bound"
        );
    }
}

pub fn read_proc_stat_bounded(path: &Path) -> std::io::Result<String> {
    use std::io::Read;
    let mut bytes = Vec::with_capacity(4096);
    fs::File::open(path)?
        .take(16 * 1024 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > 16 * 1024 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "proc stat exceeds16KiB",
        ));
    }
    String::from_utf8(bytes)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))
}
pub fn proc_start_ticks(text: &str) -> Result<u64, String> {
    if text.len() > 16 * 1024 {
        return Err("oversized proc stat".into());
    }
    let (_, fields) = text
        .rsplit_once(") ")
        .ok_or("malformed proc stat delimiter")?;
    fields
        .split_whitespace()
        .nth(19)
        .ok_or("proc stat start ticks missing")?
        .parse::<u64>()
        .map_err(|_| "proc stat start ticks invalid".into())
}
pub fn original_helper_gone(
    observed: Result<&str, std::io::ErrorKind>,
    expected_ticks: u64,
) -> Result<bool, String> {
    match observed {
        Ok(stat) => Ok(proc_start_ticks(stat)? != expected_ticks),
        Err(std::io::ErrorKind::NotFound) => Ok(true),
        Err(kind) => Err(format!(
            "original helper process identity is unknown: {kind:?}"
        )),
    }
}
#[cfg(test)]
mod proc_tests {
    use super::*;
    #[test]
    fn only_not_found_or_changed_start_ticks_prove_original_helper_exit() {
        assert_eq!(
            original_helper_gone(Err(std::io::ErrorKind::NotFound), 9),
            Ok(true)
        );
        for kind in [
            std::io::ErrorKind::PermissionDenied,
            std::io::ErrorKind::InvalidData,
            std::io::ErrorKind::Other,
        ] {
            assert!(original_helper_gone(Err(kind), 9).is_err());
        }
        assert!(original_helper_gone(Ok("malformed"), 9).is_err());
        let mut fields = vec!["0"; 20];
        fields[19] = "9";
        let stat = format!("42 (name with ) embedded) {}", fields.join(" "));
        assert_eq!(original_helper_gone(Ok(&stat), 9), Ok(false));
        assert_eq!(original_helper_gone(Ok(&stat), 8), Ok(true));
    }
}

/// Shared, plugin-free authoritative phase payload, used only after the exact
/// create-new permit has been fully synced and atomically published.
pub fn phase_event_payload(run: &Path, phase: &str) -> Result<serde_json::Value, String> {
    let ordinal = ["initial_source", "success_clone", "undo_source"]
        .iter()
        .position(|p| *p == phase)
        .ok_or("unknown launch phase")?;
    Ok(
        serde_json::json!({"kind":"phase","phase":phase,"ordinal":ordinal,"permit_path":run.join(format!("launch-permit-{phase}.json"))}),
    )
}
#[cfg(test)]
mod interface_tests {
    use super::*;
    #[test]
    fn authoritative_phase_payload_matches_shared_runner_fixture() {
        let expected: serde_json::Value =
            serde_json::from_str(include_str!("clip_render_genuine_phase_fixture.json")).unwrap();
        let actual = serde_json::Value::Array(
            ["initial_source", "success_clone", "undo_source"]
                .iter()
                .map(|p| phase_event_payload(Path::new("/synthetic/run"), p).unwrap())
                .collect(),
        );
        assert_eq!(actual, expected);
        assert!(phase_event_payload(Path::new("/synthetic/run"), "fourth_source").is_err());
    }
}
