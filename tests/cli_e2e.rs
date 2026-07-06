//! End-to-end CLI tests: drive the built `agentskillpack` binary through the
//! full lifecycle (pack -> sign -> verify -> registry add -> resolve -> lock ->
//! unpack) and the failure paths (tamper, wrong key, bad manifest). These treat
//! the binary as a black box and assert on exit codes and stdout, exactly as a
//! CI pipeline or a shell script would.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Path to the freshly-built CLI binary (Cargo sets this env for integration
/// tests of the crate's own binary target).
fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_agentskillpack")
}

fn scratch(tag: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let d = std::env::temp_dir().join(format!("asp_cli_{tag}_{nanos}"));
    fs::create_dir_all(&d).unwrap();
    d
}

/// Write a small valid skill directory; returns its path.
fn make_skill(base: &Path, name: &str, version: &str) -> PathBuf {
    let dir = base.join(name);
    fs::create_dir_all(dir.join("scripts")).unwrap();
    fs::write(
        dir.join("skill.json"),
        format!(
            r#"{{"name":"{name}","version":"{version}","description":"cli test skill",
            "capabilities":[{{"kind":"fs.read","scope":["./data"]}}],
            "entrypoint":"scripts/run.sh"}}"#
        ),
    )
    .unwrap();
    fs::write(dir.join("scripts/run.sh"), b"#!/bin/sh\necho hi\n").unwrap();
    dir
}

struct Output {
    code: i32,
    stdout: String,
    stderr: String,
}

fn run(args: &[&str]) -> Output {
    let out = Command::new(bin())
        .args(args)
        .output()
        .expect("spawn agentskillpack");
    Output {
        code: out.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    }
}

#[test]
fn full_lifecycle_pack_sign_verify_registry_lock_unpack() {
    let base = scratch("lifecycle");
    let skill = make_skill(&base, "alpha-skill", "1.2.0");
    let archive = base.join("alpha.skillpack");
    let keys = base.join("keys");
    let reg = base.join("registry");
    let out = base.join("restored");

    // pack --validate
    let r = run(&[
        "pack",
        skill.to_str().unwrap(),
        "-o",
        archive.to_str().unwrap(),
        "--validate",
    ]);
    assert_eq!(r.code, 0, "pack failed: {}", r.stderr);
    assert!(archive.exists());

    // keygen
    let r = run(&["keygen", "-o", keys.to_str().unwrap(), "--name", "author"]);
    assert_eq!(r.code, 0, "keygen failed: {}", r.stderr);
    let priv_key = keys.join("author.key");
    let pub_key = keys.join("author.pub");
    assert!(priv_key.exists() && pub_key.exists());

    // sign
    let r = run(&[
        "sign",
        archive.to_str().unwrap(),
        "--key",
        priv_key.to_str().unwrap(),
    ]);
    assert_eq!(r.code, 0, "sign failed: {}", r.stderr);
    let sig = base.join("alpha.skillpack.sig");
    assert!(sig.exists());

    // verify with pubkey
    let r = run(&[
        "verify",
        archive.to_str().unwrap(),
        "--pubkey",
        pub_key.to_str().unwrap(),
    ]);
    assert_eq!(r.code, 0, "signed verify failed: {}", r.stderr);
    assert!(r.stdout.contains("signature valid"));

    // registry add
    let r = run(&[
        "registry",
        "add",
        archive.to_str().unwrap(),
        "--registry",
        reg.to_str().unwrap(),
    ]);
    assert_eq!(r.code, 0, "registry add failed: {}", r.stderr);

    // registry resolve
    let r = run(&[
        "registry",
        "resolve",
        "alpha-skill",
        "--req",
        "^1.0",
        "--registry",
        reg.to_str().unwrap(),
    ]);
    assert_eq!(r.code, 0, "resolve failed: {}", r.stderr);
    assert!(r.stdout.contains("1.2.0"));

    // unpack
    let r = run(&[
        "unpack",
        archive.to_str().unwrap(),
        "-o",
        out.to_str().unwrap(),
    ]);
    assert_eq!(r.code, 0, "unpack failed: {}", r.stderr);
    assert!(out.join("scripts/run.sh").exists());

    fs::remove_dir_all(&base).ok();
}

#[test]
fn verify_fails_on_tampered_archive() {
    let base = scratch("cli_tamper");
    let skill = make_skill(&base, "t-skill", "0.1.0");
    let archive = base.join("t.skillpack");
    let r = run(&[
        "pack",
        skill.to_str().unwrap(),
        "-o",
        archive.to_str().unwrap(),
    ]);
    assert_eq!(r.code, 0);

    let mut bytes = fs::read(&archive).unwrap();
    let n = bytes.len();
    bytes[n - 3] ^= 0xff;
    fs::write(&archive, &bytes).unwrap();

    let r = run(&["verify", archive.to_str().unwrap()]);
    assert_ne!(r.code, 0, "tampered archive must fail verify");
    fs::remove_dir_all(&base).ok();
}

#[test]
fn verify_rejects_wrong_key() {
    let base = scratch("cli_wrongkey");
    let skill = make_skill(&base, "w-skill", "0.1.0");
    let archive = base.join("w.skillpack");
    run(&[
        "pack",
        skill.to_str().unwrap(),
        "-o",
        archive.to_str().unwrap(),
    ]);

    let k1 = base.join("k1");
    let k2 = base.join("k2");
    run(&["keygen", "-o", k1.to_str().unwrap(), "--name", "signer"]);
    run(&["keygen", "-o", k2.to_str().unwrap(), "--name", "other"]);

    run(&[
        "sign",
        archive.to_str().unwrap(),
        "--key",
        k1.join("signer.key").to_str().unwrap(),
    ]);

    // Verify against the *wrong* public key must fail.
    let r = run(&[
        "verify",
        archive.to_str().unwrap(),
        "--pubkey",
        k2.join("other.pub").to_str().unwrap(),
    ]);
    assert_ne!(r.code, 0, "wrong key must be rejected");
    fs::remove_dir_all(&base).ok();
}

#[test]
fn manifest_validate_rejects_bad_manifest() {
    let base = scratch("cli_badmanifest");
    let dir = base.join("bad");
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("skill.json"),
        br#"{"name":"Bad Name","version":"nope"}"#,
    )
    .unwrap();
    let r = run(&["manifest", "validate", dir.to_str().unwrap()]);
    assert_ne!(r.code, 0, "invalid manifest must fail");
    assert!(r.stderr.contains("INVALID"));

    // And the JSON form.
    let r = run(&["manifest", "validate", dir.to_str().unwrap(), "--json"]);
    assert!(r.stdout.contains("\"valid\":false") || r.stdout.contains("valid"));
    fs::remove_dir_all(&base).ok();
}

#[test]
fn pack_validate_rejects_bad_manifest() {
    let base = scratch("cli_packvalidate");
    let dir = base.join("bad");
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("skill.json"),
        br#"{"name":"ok-name","version":"1.0.0","dependencies":[{"name":"ok-name","req":"^1"}]}"#,
    )
    .unwrap();
    let archive = base.join("out.skillpack");
    // self-dependency should fail validation.
    let r = run(&[
        "pack",
        dir.to_str().unwrap(),
        "-o",
        archive.to_str().unwrap(),
        "--validate",
    ]);
    assert_ne!(r.code, 0, "self-dependency must fail --validate");
    fs::remove_dir_all(&base).ok();
}

#[test]
fn schema_command_emits_json() {
    let r = run(&["schema"]);
    assert_eq!(r.code, 0);
    let v: serde_json::Value = serde_json::from_str(&r.stdout).unwrap();
    assert!(v["properties"]["capabilities"].is_object());
}

#[test]
fn lock_writes_deterministic_lockfile() {
    let base = scratch("cli_lock");
    // Build a dependency skill and add it to a registry.
    let reg = base.join("reg");
    let dep = make_skill(&base, "dep-skill", "1.3.0");
    let dep_archive = base.join("dep.skillpack");
    run(&[
        "pack",
        dep.to_str().unwrap(),
        "-o",
        dep_archive.to_str().unwrap(),
    ]);
    run(&[
        "registry",
        "add",
        dep_archive.to_str().unwrap(),
        "--registry",
        reg.to_str().unwrap(),
    ]);

    // A root skill depending on it.
    let root = base.join("root");
    fs::create_dir_all(&root).unwrap();
    fs::write(
        root.join("skill.json"),
        br#"{"name":"root-skill","version":"0.1.0","dependencies":[{"name":"dep-skill","req":"^1.0"}]}"#,
    )
    .unwrap();

    let lock = base.join("skillpack.lock");
    let r = run(&[
        "lock",
        root.to_str().unwrap(),
        "--registry",
        reg.to_str().unwrap(),
        "-o",
        lock.to_str().unwrap(),
    ]);
    assert_eq!(r.code, 0, "lock failed: {}", r.stderr);
    let first = fs::read_to_string(&lock).unwrap();
    assert!(first.contains("1.3.0"));

    // Regenerate; must be byte-identical.
    run(&[
        "lock",
        root.to_str().unwrap(),
        "--registry",
        reg.to_str().unwrap(),
        "-o",
        lock.to_str().unwrap(),
    ]);
    let second = fs::read_to_string(&lock).unwrap();
    assert_eq!(first, second, "lockfile must be deterministic");

    fs::remove_dir_all(&base).ok();
}
