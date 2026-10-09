#[path = "production-src/plugins.rs"]
#[allow(dead_code)]
mod plugins;
use std::path::PathBuf;
fn main() {
    let root = std::env::var_os("VALIDATION_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::current_dir().expect("current directory"))
        .join("plugins");
    let paths=[root.join("Surge XT.vst3"),root.join("Surge XT Effects.vst3")];
    let found=plugins::scan(&paths);
    assert_eq!(found.len(),2);
    for descriptor in &found {
        assert!(descriptor.verified, "probe failed: {descriptor:?}");
        assert_eq!(descriptor.vendor,"Surge Synth Team");
        let metadata=descriptor.vst3_metadata.as_ref().unwrap();
        assert!(!metadata.class_uid.is_empty());
        println!("{descriptor:?}");
        match descriptor.name.as_str() {
            "Surge XT" => {assert!(descriptor.is_instrument); assert_eq!(descriptor.category,"Instrument"); assert!(descriptor.has_midi_input()); assert!(!descriptor.has_midi_output());},
            "Surge XT Effects" => {assert!(!descriptor.is_instrument); assert_eq!(descriptor.category,"Effect"); assert!(!descriptor.has_midi_input()); assert!(!descriptor.has_midi_output());},
            other=>panic!("Unexpected plugin {other}"),
        }
    }
    let serialized=serde_json::to_string_pretty(&found).unwrap();
    let cache=plugins::decode_cache(&serialized).unwrap();
    assert!(!cache.needs_rescan);
    let cached=cache.plugins;
    assert_eq!(cached.len(),2);
    assert_eq!(cached[0].vst3_metadata,found[0].vst3_metadata);
    assert_eq!(cached[1].vst3_metadata,found[1].vst3_metadata);
    println!("CACHE_JSON_START\n{serialized}\nCACHE_JSON_END");
    println!("PASS: actual official Surge instrument/effect classifications, event capabilities and cache round-trip");
}
