use crate::{
    model::{AudioAsset, CURRENT_PROJECT_FORMAT_VERSION, Project},
    sampler::SamplerConfig,
};
#[test]
fn v14_projects_default_to_oscillator_and_v15_sampler_roundtrips() {
    let project = Project::default();
    let mut json = serde_json::to_value(&project).unwrap();
    json["format_version"] = 14.into();
    for channel in json["channels"].as_array_mut().unwrap() {
        channel.as_object_mut().unwrap().remove("sampler");
    }
    let mut restored: Project = serde_json::from_value(json).unwrap();
    restored.normalize();
    assert_eq!(restored.format_version, CURRENT_PROJECT_FORMAT_VERSION);
    assert!(
        restored
            .channels
            .iter()
            .all(|channel| channel.sampler.is_none())
    );
    restored.audio_assets.push(AudioAsset {
        id: 900,
        name: "Sample".into(),
        path: "missing-but-relinkable.wav".into(),
        sample_rate: 48000,
        channels: 2,
        bits_per_sample: 16,
        frames: 48000,
        waveform_peaks: vec![],
    });
    restored.channels[0].sampler = Some(SamplerConfig {
        start_frame: 100,
        end_frame: Some(40000),
        loop_start_frame: Some(1000),
        loop_end_frame: Some(20000),
        ..SamplerConfig::new(900)
    });
    let json = serde_json::to_string(&restored).unwrap();
    let reopened: Project = serde_json::from_str(&json).unwrap();
    assert_eq!(reopened.channels[0].sampler, restored.channels[0].sampler);
    assert_eq!(reopened.audio_assets[0].path, restored.audio_assets[0].path);
}
#[test]
fn sampler_only_asset_survives_save_and_missing_media_reopen() {
    let mut project = Project::default();
    project.audio_assets.push(AudioAsset {
        id: 901,
        name: "Offline".into(),
        path: "missing-sampler.wav".into(),
        sample_rate: 48000,
        channels: 1,
        bits_per_sample: 16,
        frames: 480,
        waveform_peaks: vec![],
    });
    project.channels[0].sampler = Some(SamplerConfig::new(901));
    let path =
        std::env::temp_dir().join(format!("citrus-sampler-{}-save.json", std::process::id()));
    project.save(&path).unwrap();
    let reopened = Project::load(&path).unwrap();
    std::fs::remove_file(&path).unwrap();
    assert_eq!(reopened.channels[0].sampler, project.channels[0].sampler);
    assert_eq!(reopened.audio_assets[0].id, 901);
}
#[test]
fn invalid_sampler_settings_cannot_be_saved() {
    let mut project = Project::default();
    project.channels[0].sampler = Some(SamplerConfig {
        root_note: 255,
        ..SamplerConfig::new(900)
    });
    let path = std::env::temp_dir().join(format!(
        "citrus-sampler-{}-invalid.json",
        std::process::id()
    ));
    assert!(project.save(&path).is_err());
    assert!(!path.exists());
}

#[test]
fn empty_sampler_is_distinct_from_legacy_and_survives_save_reopen() {
    let mut project = Project::default();
    project.channels[0].sampler = Some(SamplerConfig::empty());
    let path =
        std::env::temp_dir().join(format!("citrus-empty-sampler-{}.json", std::process::id()));
    project.save(&path).unwrap();
    let saved = std::fs::read_to_string(&path).unwrap();
    let json: serde_json::Value = serde_json::from_str(&saved).unwrap();
    assert!(json["channels"][0]["sampler"]["asset_id"].is_null());
    let restored = Project::load(&path).unwrap();
    std::fs::remove_file(path).unwrap();
    assert_eq!(restored.channels[0].sampler, Some(SamplerConfig::empty()));
    assert_eq!(restored.channels[1].sampler, None);
    let old: SamplerConfig = serde_json::from_str("{\"asset_id\":99}").unwrap();
    assert_eq!(old.asset_id, Some(99));
}
