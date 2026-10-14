//! Opt-in, device-free project persistence/export checks for the local Kontakt probe.
//! This generates a muted default-Kontakt project; it does not load or qualify NOIRE.

use std::{collections::BTreeMap, fs, path::PathBuf};

use crate::{
    export,
    mixer_graph::MASTER_MIXER_TRACK_ID,
    model::{Pattern, PianoNote, PluginFormat, PluginInstance, PluginRole, Project},
};

#[path = "windows_kontakt_runtime_qa.rs"]
mod runtime_qa;

#[test]
#[ignore = "requires CITRUS_KONTAKT_QA_DIR from the explicit local Kontakt helper probe"]
fn windows_kontakt_muted_project_save_reopen_and_export_boundary() {
    let root = PathBuf::from(
        std::env::var_os("CITRUS_KONTAKT_QA_DIR").expect("set CITRUS_KONTAKT_QA_DIR"),
    );
    assert!(
        root.is_absolute(),
        "QA root must be an absolute private path"
    );
    let probe: serde_json::Value = serde_json::from_slice(
        &fs::read(root.join("helper-default-instance/result.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(probe["status"], "passed", "complete the helper probe first");
    let state = fs::read(root.join("helper-default-instance/default-kontakt.state.bin")).unwrap();
    assert!(!state.is_empty());
    let output = root.join("project-model-check");
    // Refuse to overwrite any user project or a previous QA run.
    fs::create_dir(&output).expect("use a fresh project-model-check directory");

    let mut project = Project {
        name: "Kontakt default QA - NOIRE not loaded - MASTER MUTED".into(),
        tempo: 80.0,
        swing: 0.0,
        song_length_beats: 16.0,
        active_pattern: 0,
        ..Project::default()
    };
    project.automation_lanes.clear();
    project.audio_assets.clear();
    project.mixer_insert_slots.clear();
    project.audio_clip_mixer_destinations.clear();
    let mut channel = project.channels[4].clone();
    channel.id = 1;
    channel.name = "Kontakt 8 - NOIRE selection pending".into();
    channel.steps = [false; 16];
    channel.volume = 0.05;
    channel.mixer_track = MASTER_MIXER_TRACK_ID;
    channel.instrument_plugin_instance_id = Some(1);
    project.channels = vec![channel];
    let notes = [60, 64, 67, 71, 69, 67, 64, 60]
        .into_iter()
        .enumerate()
        .map(|(index, note)| PianoNote {
            id: index as u64 + 1,
            channel_id: Some(1),
            group_id: None,
            note,
            start: index as f32 * 2.0,
            length: 1.0,
            velocity: 0.2,
            selected: false,
            muted: false,
        })
        .collect::<Vec<_>>();
    project.patterns = vec![Pattern {
        id: 1,
        name: "Four-bar QA melody".into(),
        length_steps: 64,
        channel_steps: vec![[false; 16]],
        notes,
    }];
    project.clips.truncate(1);
    let clip = &mut project.clips[0];
    clip.id = 1;
    clip.track = 0;
    clip.start = 0.0;
    clip.length = 16.0;
    clip.name = "QA melody - default Kontakt state".into();
    clip.pattern_id = 1;
    project.plugin_instances = vec![PluginInstance {
        midi_ports: Default::default(),
        id: 1,
        format: PluginFormat::Vst3,
        role: PluginRole::Instrument,
        path: PathBuf::from(probe["plugin"].as_str().unwrap()),
        uid: probe["plugin_info"]["uid"].as_str().unwrap().into(),
        vendor: "Native Instruments".into(),
        name: "Kontakt 8".into(),
        enabled: true,
        bypass: false,
        wet: 1.0,
        parameters: BTreeMap::new(),
        opaque_state: state.clone(),
        runtime_status: Default::default(),
    }];
    let master = project
        .mixer_tracks
        .iter_mut()
        .find(|track| track.id == MASTER_MIXER_TRACK_ID)
        .unwrap();
    master.muted = true;
    master.volume = 0.0;
    project.normalize();
    let original_path = output.join("Kontakt-default-MUTED.citrus");
    let reopened_path = output.join("Kontakt-default-MUTED-reopened.citrus");
    assert!(!original_path.exists() && !reopened_path.exists());
    project.save(&original_path).unwrap();
    let reopened = Project::load(&original_path).unwrap();
    assert_eq!(reopened.plugin_instances[0].opaque_state, state);
    assert_eq!(reopened.plugin_instances, project.plugin_instances);
    assert_eq!(
        serde_json::to_value(&reopened.patterns).unwrap(),
        serde_json::to_value(&project.patterns).unwrap(),
    );
    assert_eq!(
        serde_json::to_value(&reopened.channels).unwrap(),
        serde_json::to_value(&project.channels).unwrap(),
    );
    let master = reopened
        .mixer_tracks
        .iter()
        .find(|track| track.id == MASTER_MIXER_TRACK_ID)
        .unwrap();
    assert!(master.muted);
    assert_eq!(master.volume, 0.0);
    reopened.save(&reopened_path).unwrap();
    let twice = Project::load(&reopened_path).unwrap();
    assert_eq!(twice.plugin_instances[0].opaque_state, state);
    let export_path = output.join("active-kontakt-must-not-export.wav");
    let refusal = export::render_project_wav(&twice, &export_path, 48_000)
        .expect_err("active VST song rendering is deliberately unsupported");
    let reason = refusal.to_string();
    assert!(reason.contains("active instrument plug-in"), "{reason}");
    assert!(
        !export_path.exists(),
        "refused export must not publish audio"
    );
    let result = serde_json::json!({
        "status": "passed",
        "scope": "production Project save/load and offline export refusal; no native App UI, device, or NOIRE instrument acceptance",
        "project": original_path,
        "resaved_project": reopened_path,
        "format_version": twice.format_version,
        "notes": twice.patterns[0].notes.len(),
        "opaque_state_bytes": state.len(),
        "master_muted": true,
        "master_volume": 0.0,
        "vst_offline_song_export": "not implemented",
        "export_refusal": reason,
        "export_destination_created": false,
        "noire_loaded": false,
        "native_save_close_reopen": "not tested",
        "physical_audio": "not tested",
    });
    fs::write(
        output.join("project-result.json"),
        serde_json::to_vec_pretty(&result).unwrap(),
    )
    .unwrap();
    println!("{}", serde_json::to_string_pretty(&result).unwrap());
}
