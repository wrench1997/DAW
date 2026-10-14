//! Declaration-only indexing and cache semantics; no SDK, devices or user files.
use super::*;

pub(super) fn catalog() -> Vst3FactoryCatalog {
    let first = Vst3FactoryClass {
        class_id: "0123456789ABCDEF0123456789ABCDEF".into(),
        name: "First declared controller, not a default instrument".into(),
        category: "Component Controller Class".into(),
        cardinality: i32::MAX,
        vendor: None,
        version: None,
        sdk_version: None,
        sub_categories: None,
        class_flags: None,
    };
    let mut second = first.clone();
    second.class_id = "FEDCBA9876543210FEDCBA9876543210".into();
    second.name = "Declared instrument, not loaded".into();
    second.category = "Audio Module Class".into();
    second.vendor = Some(String::new());
    second.sub_categories = Some(vec!["Instrument".into()]);
    second.version = Some("8.4.1".into());
    second.class_flags = Some(0);
    Vst3FactoryCatalog {
        factory: Vst3FactoryInfo {
            vendor: "Factory vendor".into(),
            url: String::new(),
            email: String::new(),
            flags: -1,
        },
        classes: vec![first, second],
    }
}

#[test]
fn catalog_scan_indexes_one_module_without_selecting_a_declared_default() {
    let dir = super::tests::TestDirectory::new();
    let bundle = dir.0.join("Multi Synth FX.vst3");
    std::fs::create_dir_all(bundle.join("Contents/Incorrect.vst3")).unwrap();
    let expected = catalog();
    let mut calls = Vec::new();
    let mut probe = |path: &Path| {
        calls.push(path.to_path_buf());
        Ok(Vst3CatalogProbeInfo {
            catalog: expected.clone(),
            lifecycle: None,
        })
    };
    let ScanOutcome::Completed(report) = scan_catalog_with_probe(
        &[dir.0.clone(), bundle.clone()],
        &AtomicBool::new(false),
        &mut probe,
    ) else {
        panic!("synthetic catalog scan stopped")
    };
    assert_eq!(calls, vec![bundle.clone()]);
    assert_eq!(report.plugins.len(), 1);
    let plugin = &report.plugins[0];
    assert_eq!(plugin.path, bundle);
    assert_eq!(plugin.id, format!("VST3:{}", bundle.display()));
    assert_eq!(plugin.name, "Multi Synth FX");
    assert_eq!(plugin.vendor, "Factory vendor");
    assert!(plugin.is_catalog_only());
    assert!(!plugin.verified && !plugin.is_instrument);
    assert_eq!(plugin.category, "Unknown");
    assert!(plugin.vst3_metadata.is_none() && plugin.scan_error.is_none());
    assert!(!plugin.has_midi_input() && !plugin.has_midi_output());
    assert_eq!(plugin.vst3_catalog.as_ref(), Some(&expected));
}

#[test]
fn catalog_cache_round_trip_preserves_factory_order_unknown_and_empty_getters() {
    let mut value = catalog();
    value.classes[1].sub_categories = Some(Vec::new());
    let plugin = vst3_catalog_descriptor(Path::new("Multi.vst3"), value.clone());
    let encoded = serde_json::to_string(&vec![plugin.clone()]).unwrap();
    let cache = decode_cache(&encoded).unwrap();
    assert!(!cache.needs_rescan);
    assert_eq!(cache.plugins.len(), 1);
    let restored = &cache.plugins[0];
    assert!(restored.is_catalog_only());
    assert_eq!(restored.id, plugin.id);
    assert_eq!(restored.path, plugin.path);
    assert_eq!(restored.vst3_catalog, Some(value));
    let classes = &restored.vst3_catalog.as_ref().unwrap().classes;
    assert_eq!(classes[0].vendor, None);
    assert_eq!(classes[1].vendor.as_deref(), Some(""));
    assert_eq!(classes[1].sub_categories, Some(Vec::new()));
}

#[test]
fn catalog_cache_cannot_forge_runtime_capabilities_or_admit_invalid_declarations() {
    for scenario in 0..7 {
        let mut plugin = vst3_catalog_descriptor(Path::new("Multi.vst3"), catalog());
        match scenario {
            0 => plugin.verified = true,
            1 => plugin.is_instrument = true,
            2 => plugin.category = "Instrument".into(),
            3 => plugin.vst3_catalog.as_mut().unwrap().classes[0].class_id = "bad".into(),
            4 => plugin.vst3_catalog.as_mut().unwrap().classes[0].class_id = "0".repeat(32),
            5 => {
                plugin.vst3_catalog.as_mut().unwrap().classes[0].sub_categories =
                    Some(vec![String::new(); 257])
            }
            6 => {
                plugin.vst3_catalog.as_mut().unwrap().classes =
                    vec![catalog().classes[0].clone(); 4097]
            }
            _ => unreachable!(),
        }
        let cache = decode_cache(&serde_json::to_string(&vec![plugin]).unwrap()).unwrap();
        assert!(
            cache.needs_rescan && cache.plugins.is_empty(),
            "scenario {scenario}"
        );
    }
}

#[test]
fn additive_catalog_keeps_loaded_metadata_vst2_and_legacy_cache_contracts() {
    let catalog = vst3_catalog_descriptor(Path::new("Catalog.vst3"), catalog());
    let loaded = vst3_descriptor(
        Path::new("Loaded.vst3"),
        Ok(super::tests::metadata("Fx", true, false)),
    );
    let vst2 = descriptor_from_path(Path::new("Old.dll"), PluginFormat::Vst2, true);
    let legacy = descriptor_from_path(Path::new("Legacy.vst3"), PluginFormat::Vst3, true);
    let cache = decode_cache(&serde_json::to_string(&vec![catalog, loaded, vst2, legacy]).unwrap())
        .unwrap();
    assert!(cache.needs_rescan);
    assert_eq!(cache.plugins.len(), 3);
    assert!(cache.plugins[0].is_catalog_only());
    assert!(cache.plugins[1].verified && cache.plugins[1].has_midi_input());
    assert!(cache.plugins[1].vst3_catalog.is_none());
    assert_eq!(cache.plugins[2].format, PluginFormat::Vst2);
    assert!(cache.plugins[2].verified);
}

#[test]
fn clean_reaped_catalog_metadata_failure_keeps_unknown_error_and_continues_without_fallback() {
    let dir = super::tests::TestDirectory::new();
    for name in ["One.vst3", "Two.vst3"] {
        std::fs::create_dir(dir.0.join(name)).unwrap();
    }
    let mut calls = Vec::new();
    let mut probe = |path: &Path| {
        calls.push(path.to_path_buf());
        let mut receipt = super::tests::synthetic_receipt(path, 300 + calls.len() as u32);
        if calls.len() == 1 {
            receipt.load_failure = Some("synthetic catalog refusal".into());
            let mut failure = ProbeFailure::metadata("synthetic catalog refusal");
            failure.lifecycle = Some(Box::new(receipt));
            Err(failure)
        } else {
            Ok(Vst3CatalogProbeInfo {
                catalog: catalog(),
                lifecycle: Some(receipt),
            })
        }
    };
    let ScanOutcome::Completed(report) = scan_catalog_with_probe(
        std::slice::from_ref(&dir.0),
        &AtomicBool::new(false),
        &mut probe,
    ) else {
        panic!("healthy per-module refusal stopped the entire catalog scan")
    };
    assert_eq!(calls.len(), 2);
    assert_eq!(report.probe_lifecycle.len(), 2);
    let failed = report
        .plugins
        .iter()
        .find(|plugin| plugin.path == calls[0])
        .unwrap();
    assert!(!failed.verified && !failed.is_instrument);
    assert_eq!(failed.category, "Unknown");
    assert!(failed.vst3_catalog.is_none() && failed.vst3_metadata.is_none());
    assert_eq!(
        failed.scan_error.as_deref(),
        Some("synthetic catalog refusal")
    );
    let succeeded = report
        .plugins
        .iter()
        .find(|plugin| plugin.path == calls[1])
        .unwrap();
    assert!(succeeded.is_catalog_only());
    let cache = decode_cache(&serde_json::to_string(&report.plugins).unwrap()).unwrap();
    assert!(!cache.needs_rescan && cache.plugins.len() == 2);
    assert!(
        cache
            .plugins
            .iter()
            .any(|plugin| plugin.scan_error.is_some())
    );
}

#[test]
fn unclean_or_cancelled_catalog_scan_stops_without_publishing_partial_or_replaying_a_probe() {
    for scenario in 1..5 {
        let dir = super::tests::TestDirectory::new();
        for name in ["One.vst3", "Two.vst3", "Three.vst3"] {
            std::fs::create_dir(dir.0.join(name)).unwrap();
        }
        let mut calls = 0;
        let mut probe = |path: &Path| {
            calls += 1;
            let mut receipt = super::tests::synthetic_receipt(path, calls);
            if calls == 1 {
                return Ok(Vst3CatalogProbeInfo {
                    catalog: catalog(),
                    lifecycle: Some(receipt),
                });
            }
            let mut failure = ProbeFailure::metadata("synthetic catalog refusal");
            let kind = match scenario {
                1 => {
                    receipt.kill_requested = true;
                    ScanStopKind::UncleanRetirement
                }
                2 => {
                    receipt.exit_success = false;
                    receipt.exit_code = Some(9);
                    ScanStopKind::UncleanRetirement
                }
                3 => {
                    receipt.reaped = false;
                    receipt.exit_success = false;
                    receipt.exit_code = None;
                    ScanStopKind::CleanupPending
                }
                4 => ScanStopKind::Cancelled,
                _ => unreachable!(),
            };
            failure.stopped = Some(ScanStopped::new(kind, "synthetic stop"));
            failure.lifecycle = Some(Box::new(receipt));
            Err(failure)
        };
        let ScanOutcome::Stopped(stopped) = scan_catalog_with_probe(
            std::slice::from_ref(&dir.0),
            &AtomicBool::new(false),
            &mut probe,
        ) else {
            panic!("failed catalog replaced previous index")
        };
        assert_eq!(calls, 2);
        assert_eq!(stopped.probe_lifecycle.len(), 2);
    }
}

#[test]
fn catalog_cancellation_after_last_response_keeps_receipt_not_completed_index() {
    let dir = super::tests::TestDirectory::new();
    let bundle = dir.0.join("One.vst3");
    std::fs::create_dir(&bundle).unwrap();
    let cancelled = AtomicBool::new(false);
    let mut calls = 0;
    let mut probe = |path: &Path| {
        calls += 1;
        cancelled.store(true, Ordering::Release);
        Ok(Vst3CatalogProbeInfo {
            catalog: catalog(),
            lifecycle: Some(super::tests::synthetic_receipt(path, 17)),
        })
    };
    let ScanOutcome::Stopped(stopped) =
        scan_catalog_with_probe(std::slice::from_ref(&bundle), &cancelled, &mut probe)
    else {
        panic!("late cancellation published catalog")
    };
    assert_eq!(calls, 1);
    assert!(matches!(stopped.kind, ScanStopKind::Cancelled));
    assert_eq!(stopped.probe_lifecycle.len(), 1);
}
