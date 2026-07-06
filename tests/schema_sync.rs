//! Guard test: the committed `docs/skill.schema.json` must byte-match what the
//! `manifest::json_schema()` generator produces, so the shipped schema can
//! never drift from the code. If this fails, regenerate the file with
//! `cargo run -- schema > docs/skill.schema.json`.

use agentskillpack::manifest::json_schema;

#[test]
fn committed_schema_matches_generator() {
    let generated = json_schema();
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/docs/skill.schema.json");
    let on_disk = std::fs::read_to_string(path).expect("docs/skill.schema.json exists");
    assert_eq!(
        generated.trim(),
        on_disk.trim(),
        "docs/skill.schema.json is stale — regenerate with `cargo run -- schema`"
    );
}

#[test]
fn generated_schema_is_parseable_json() {
    let v: serde_json::Value = serde_json::from_str(&json_schema()).unwrap();
    assert!(v["$schema"].as_str().unwrap().contains("json-schema.org"));
}
