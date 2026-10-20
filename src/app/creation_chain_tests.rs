//! Full built-in melody workflow driven through production egui input and save/export paths.
//! Native dialogs, physical audio and independent OS-process startup are outside this test.
use super::*;
use crate::export_job::ExportControl;
use crate::export_options::{WavExportOptions, WavLevelPolicy, WavSampleFormat};

/// Retain synthetic artifacts only when explicitly requested by the test operator.
struct EvidenceDirectory {
    path: PathBuf,
    retained: bool,
}

impl std::ops::Deref for EvidenceDirectory {
    type Target = std::path::Path;
    fn deref(&self) -> &Self::Target {
        &self.path
    }
}

impl Drop for EvidenceDirectory {
    fn drop(&mut self) {
        if !self.retained {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }
}

fn evidence(label: &str) -> EvidenceDirectory {
    if let Some(root) = std::env::var_os("CITRUS_CREATION_CHAIN_EVIDENCE_DIR") {
        let path = PathBuf::from(root).join(label);
        std::fs::create_dir_all(&path).unwrap();
        return EvidenceDirectory {
            path,
            retained: true,
        };
    }
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!(
        "citrus-creation-chain-{}-{stamp}-{label}",
        std::process::id()
    ));
    std::fs::create_dir(&path).unwrap();
    EvidenceDirectory {
        path,
        retained: false,
    }
}

fn options() -> WavExportOptions {
    WavExportOptions {
        sample_rate: 48_000,
        sample_format: WavSampleFormat::Float32,
        level_policy: WavLevelPolicy::PreserveLevel,
        tail_milliseconds: 250,
    }
}
fn setup() -> UiHarness {
    let mut ui = UiHarness::new();
    let mut project = Project::blank();
    project.name = "Strict creation acceptance".into();
    project.tempo = 120.0;
    project.song_length_beats = 16.0;
    project.auto_song_length = false;
    assert!(ui.app.install_project(project, None, false));
    ui.app.piano_roll_state.local_snap = PianoSnap::QuarterBeat;
    ui.app.piano_roll_state.last_note_length = 0.5;
    ui.app.piano_roll_state.tool = PianoRollTool::Draw;
    ui.app.tool_mode = ToolMode::Draw;
    ui.app.snap = 0.25;
    ui.key(egui::Key::F7, egui::Modifiers::NONE);
    ui.app
        .piano_viewport
        .y
        .reveal(piano_pitch_row(73), piano_pitch_row(59) + 1.0, 12.0)
        .unwrap();
    ui.settle();
    for (beat, pitch) in [(0.0, 60), (1.0, 64), (2.0, 67), (3.0, 72)] {
        let grid = ui.ctx.read_response(Id::new("piano-grid")).unwrap().rect;
        let pos = Pos2::new(
            grid.left() + ui.app.piano_viewport.x.pixel_for_content(beat + 0.0625) as f32,
            grid.top()
                + ui.app
                    .piano_viewport
                    .y
                    .pixel_for_content(piano_pitch_row(pitch) + 0.5) as f32,
        );
        assert!(grid.contains(pos));
        ui.click_pos(pos);
    }
    let notes = &ui.app.project.active_pattern().notes;
    assert_eq!(notes.len(), 4);
    assert_eq!(
        notes
            .iter()
            .map(|n| (n.start, n.note, n.length, n.channel_id))
            .collect::<Vec<_>>(),
        vec![
            (0.0, 60, 0.5, Some(1)),
            (1.0, 64, 0.5, Some(1)),
            (2.0, 67, 0.5, Some(1)),
            (3.0, 72, 0.5, Some(1))
        ]
    );
    ui.capture("strict-authored-melody");
    ui.key(egui::Key::F5, egui::Modifiers::NONE);
    for beat in [0.0625, 8.0625] {
        let grid = ui
            .ctx
            .read_response(Id::new("playlist-canvas"))
            .unwrap()
            .rect;
        let pos = Pos2::new(
            grid.left() + ui.app.playlist_viewport.x.pixel_for_content(beat) as f32,
            grid.top() + ui.app.playlist_viewport.y.pixel_for_content(0.5) as f32,
        );
        assert!(grid.contains(pos));
        ui.click_pos(pos);
    }
    let clips = &ui.app.project.clips;
    assert_eq!(clips.len(), 2);
    assert_eq!(
        clips
            .iter()
            .map(|c| (c.start, c.length, c.pattern_id))
            .collect::<Vec<_>>(),
        vec![(0.0, 4.0, 1), (8.0, 4.0, 1)]
    );
    assert!(ui.app.dirty);
    ui
}
fn create_automation(ui: &mut UiHarness, channel: bool) {
    ui.key(
        if channel {
            egui::Key::F6
        } else {
            egui::Key::F9
        },
        egui::Modifiers::NONE,
    );
    let label = if channel {
        "Channel 1 · Volume"
    } else {
        "Master · Volume"
    };
    let node = ui.nodes.iter().find(|n| n.label() == Some(label)).unwrap();
    let pos = node_rect(node).center();
    for pressed in [true, false] {
        ui.run(vec![
            egui::Event::PointerMoved(pos),
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Secondary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            },
        ]);
    }
    ui.settle();
    ui.click("Create/open automation clip");
    ui.key(egui::Key::F5, egui::Modifiers::NONE);
    ui.click("SHAPES");
    ui.click("Fall");
    assert_eq!(ui.app.project.automation_lanes.len(), 1);
    assert_eq!(
        ui.app.project.automation_lanes[0].lane.target(),
        &if channel {
            AutomationTarget::ChannelVolume { channel: 1 }
        } else {
            AutomationTarget::MasterVolume
        }
    );
}

#[test]
fn creation_chain_melody_master_automation_save_new_app_open_export() {
    let dir = evidence("supported-chain");
    let mut ui = setup();
    create_automation(&mut ui, false);
    let edited = project_fingerprint(&ui.app.project);
    let undo_len = ui.app.undo_stack.len();
    ui.key(egui::Key::Z, piano_clipboard_command());
    assert_ne!(project_fingerprint(&ui.app.project), edited);
    ui.key(egui::Key::Y, piano_clipboard_command());
    assert_eq!(project_fingerprint(&ui.app.project), edited);
    assert_eq!(ui.app.undo_stack.len(), undo_len);
    ui.capture("strict-arrangement-master-automation");
    let before = serde_json::to_value(&ui.app.project).unwrap();
    let expected = dir.join("before-save.wav");
    let report = crate::export::render_project_wav_with_options(
        &ui.app.project,
        &expected,
        options(),
        &ExportControl::default(),
    )
    .unwrap();
    assert_eq!(report.music_frames, 384_000);
    assert_eq!(report.tail_frames, 12_000);
    assert_eq!(report.frames, 396_000);
    let decoded = crate::wav::read_wav(&expected).unwrap();
    assert_eq!(decoded.metadata.sample_rate, 48_000);
    assert_eq!(decoded.metadata.channels, 2);
    assert_eq!(decoded.samples.len(), 792_000);
    assert!(
        decoded
            .samples
            .iter()
            .all(|s| s.is_finite() && s.abs() <= 1.0)
    );
    for start in [
        0usize, 24_000, 48_000, 72_000, 192_000, 216_000, 240_000, 264_000,
    ] {
        let peak = decoded.samples[start * 2..(start + 12_000) * 2]
            .iter()
            .map(|s| s.abs())
            .fold(0.0_f32, f32::max);
        assert!(
            peak > 0.0001,
            "missing audible melody event at frame {start}: peak={peak}"
        );
    }
    let mut unity = ui.app.project.clone();
    unity.automation_lanes[0].lane.set_enabled(false);
    let reference = dir.join("automation-disabled.wav");
    crate::export::render_project_wav_with_options(
        &unity,
        &reference,
        options(),
        &ExportControl::default(),
    )
    .unwrap();
    let plain = crate::wav::read_wav(&reference).unwrap();
    let changed = decoded
        .samples
        .iter()
        .zip(&plain.samples)
        .filter(|(a, b)| (*a - *b).abs() > 1e-6)
        .count();
    assert!(
        changed > 1000,
        "automation must change actual final PCM: {changed}"
    );
    let project_path = dir.join("authored.citrus");
    ui.app.begin_project_save(ProjectSaveRequest::Manual {
        path: project_path.clone(),
        lifecycle: None,
    });
    ui.wait_until(|app| app.save_barrier.is_none());
    assert_eq!(ui.app.saved_path.as_ref(), Some(&project_path));
    assert!(!ui.app.project_is_dirty_now());
    assert_eq!(serde_json::to_value(&ui.app.project).unwrap(), before);
    drop(ui);
    let mut cold = UiHarness::new();
    cold.app.commit_project_intent(ProjectIntent::Open {
        path: project_path.clone(),
    });
    cold.settle();
    assert_eq!(cold.app.saved_path.as_ref(), Some(&project_path));
    assert_eq!(serde_json::to_value(&cold.app.project).unwrap(), before);
    assert!(!cold.app.project_is_dirty_now());
    assert!(cold.app.undo_stack.is_empty());
    // Exercise the production worker/controller and renderer with explicit options.
    // The native picker and final confirmation button/options handoff are not exercised.
    cold.menu("Export WAV…");
    cold.click("Review…");
    assert!(!cold.button("Back").is_disabled());
    cold.key(egui::Key::Escape, egui::Modifiers::NONE);
    let output = dir.join("after-cold-open.wav");
    let worker_path = output.clone();
    let project = cold.app.project.clone();
    assert!(
        cold.app
            .export_job
            .start(cold.app.project_session, output.clone(), move |control| {
                match crate::export::render_project_wav_with_options(
                    &project,
                    &worker_path,
                    options(),
                    &control,
                ) {
                    Ok(report) => ExportOutcome::Complete {
                        path: worker_path,
                        level_summary: report.level_summary(),
                    },
                    Err(error) => ExportOutcome::Failed(format!("{error:#}")),
                }
            })
            .unwrap()
    );
    cold.wait_until(|app| !app.export_job.is_running());
    assert!(
        cold.app.export_error.is_none(),
        "{:?}",
        cold.app.export_error
    );
    assert_eq!(
        std::fs::read(&output).unwrap(),
        std::fs::read(&expected).unwrap(),
        "all final WAV bytes must survive opening in a fresh App object"
    );
    assert_eq!(serde_json::to_value(&cold.app.project).unwrap(), before);
    println!(
        "STRICT_CHAIN: 4 UI-authored notes; 2 UI-authored placements; UI Master Fall automation; Undo/Redo exact; manual App save; fresh App open; 396000 frames / 792000 finite stereo samples; {changed} samples changed by automation; every WAV byte identical after cold open"
    );
}

#[test]
fn strict_creation_delete_pattern_must_preserve_unrelated_global_automation() {
    let dir = evidence("delete-global-automation");
    let mut ui = setup();
    // Valid v14 project: lanes without placements are explicitly global in the production runtime.
    let mut lane = AutomationLane::new(AutomationTarget::Tempo);
    lane.replace_points([
        AutomationPoint::new(0.0, 120.0),
        AutomationPoint::new(16.0, 90.0),
    ]);
    ui.app
        .project
        .automation_lanes
        .push(crate::model::ProjectAutomation {
            id: 500,
            name: "Global tempo: preserve this".into(),
            lane,
        });
    let mut volume = AutomationLane::new(AutomationTarget::MasterVolume);
    volume.replace_points([
        AutomationPoint::new(0.0, 0.8),
        AutomationPoint::new(16.0, 0.2),
    ]);
    ui.app
        .project
        .automation_lanes
        .push(crate::model::ProjectAutomation {
            id: 501,
            name: "Global volume: preserve this".into(),
            lane: volume,
        });
    ui.app.sync_history_observer();
    ui.app.project_fingerprint = project_fingerprint(&ui.app.project);
    ui.app.undo_stack.clear();
    ui.app.redo_stack.clear();
    ui.app.dirty = false;
    let before = serde_json::to_value(&ui.app.project.automation_lanes).unwrap();
    let path = dir.join("before.citrus");
    ui.app.project.save(&path).unwrap();
    let loaded = Project::load(&path).unwrap();
    assert_eq!(
        serde_json::to_value(&loaded.automation_lanes).unwrap(),
        before,
        "global lanes must survive ordinary validation/load"
    );
    crate::export::ensure_supported_project(&loaded).unwrap();
    ui.app.tool_mode = ToolMode::Select;
    ui.settle();
    let id = ui.app.project.clips[1].id;
    let center = ui
        .ctx
        .read_response(Id::new(("playlist-clip", id)))
        .unwrap()
        .rect
        .center();
    ui.click_pos(center);
    assert_eq!(ui.app.playlist_selection_ids, HashSet::from([id]));
    ui.key(egui::Key::Delete, egui::Modifiers::NONE);
    assert_eq!(ui.app.project.clips.len(), 1);
    let after = serde_json::to_value(&ui.app.project.automation_lanes).unwrap();
    ui.app
        .project
        .save(&dir.join("after-delete.citrus"))
        .unwrap();
    println!(
        "STRICT_DELETE: global lanes before={}, after={} (only a Pattern clip was selected)",
        before.as_array().unwrap().len(),
        after.as_array().unwrap().len()
    );
    ui.capture("strict-pattern-delete-lost-global-automation");
    ui.key(egui::Key::Z, piano_clipboard_command());
    assert_eq!(
        serde_json::to_value(&ui.app.project.automation_lanes).unwrap(),
        before,
        "one Undo must recover the deleted lanes"
    );
    assert_eq!(ui.app.project.clips.len(), 2);
    ui.key(egui::Key::Y, piano_clipboard_command());
    assert_eq!(
        serde_json::to_value(&ui.app.project.automation_lanes).unwrap(),
        after
    );
    assert_eq!(
        after, before,
        "Deleting an unrelated Pattern clip must not erase valid global Tempo/MasterVolume automation"
    );
}
