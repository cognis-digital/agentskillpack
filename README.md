# agentskillpack

**A portable, signed, capability-declaring packaging format and registry for
AI-agent skills.**

An agent *skill* is a directory of files — scripts, prompts, templates, data —
described by a `skill.json` manifest. Today those skills are usually passed
around as loose folders or unsigned zips. That is unsafe in four specific ways:

- **No integrity.** A zip can be silently corrupted or altered in transit; the
  consumer has no way to tell.
- **No provenance.** You cannot prove *who* produced a skill or that it was not
  swapped for a look-alike.
- **No capability transparency.** Nothing tells the host that a skill intends to
  hit the network, write files, or spawn processes — until it already has.
- **No version pinning.** "Install the latest research-skill" is not
  reproducible; there is no lockfile tying a build to exact bytes.

`agentskillpack` fixes all four with one small, dependency-light Rust tool:

- **`.skillpack`** — a self-describing, big-endian, per-file SHA-256 container.
  Lossless round-trip; `verify` is a CI gate.
- **Typed manifest** — a versioned `skill.json` model with **declared
  capabilities** (`fs.read`/`fs.write`/`net`/`exec`/`env` + scopes), semver
  dependencies, typed I/O, and engine compat — plus a generated **JSON Schema**.
- **ed25519 signing** — detached signatures over the whole archive, with an
  honest TOFU / bring-your-own-key trust model. Tamper and wrong-key are
  rejected.
- **Semver resolution + lockfile** — pin dependencies to the highest compatible
  version and emit a deterministic `skillpack.lock`.
- **Local registry** — an inspectable, offline, filesystem-backed store that
  verifies integrity on add and on resolve.

No external services, no network calls. Dependencies: `serde`, `serde_json`,
`sha2`, `semver`, `ed25519-dalek`.

License: COCL 1.0. Maintainer: Cognis Digital.

---

## Install (Windows / macOS / Linux)

Build from source (requires a Rust toolchain, 1.70+):

```sh
cargo build --release
# binary at target/release/agentskillpack(.exe)
```

Or use the platform scripts, which build and copy the binary onto your `PATH`:

```sh
# macOS / Linux
./install.sh

# Windows (PowerShell)
./install.ps1
```

Or via `make`:

```sh
make build     # release build
make test      # cargo test
make lint      # clippy -D warnings + fmt --check
make demo      # run the demo suite
make install   # cargo install --path .
```

Or with Docker (multi-stage, static-ish release image):

```sh
docker build -t agentskillpack .
docker run --rm -v "$PWD:/work" -w /work agentskillpack verify my-skill.skillpack
```

---

## Quickstart

Real captured output from the release binary:

```console
$ agentskillpack pack examples/research-skill -o research.skillpack --validate
packed 'research-skill' v1.4.2 — 4 file(s), 4331 bytes -> research.skillpack

$ agentskillpack manifest validate examples/research-skill
OK: 'research-skill' v1.4.2 is a valid manifest (3 capability, 1 dependency)

$ agentskillpack keygen -o keys --name author
generated ed25519 keypair:
  private: keys/author.key
  public:  keys/author.pub
  pubkey:  5cff79b5cb9eba6684b10b28eacff3771759b543bad291c32b963a264e3ad3fc
keep the private key secret; distribute the public key to verifiers.

$ agentskillpack sign research.skillpack --key keys/author.key
signed research.skillpack -> research.skillpack.sig (signer 5cff79b5cb9eba66)

$ agentskillpack verify research.skillpack --pubkey keys/author.pub
OK: 4 file(s) verified; signature valid (5cff79b5cb9eba66)

$ agentskillpack registry add research.skillpack --registry ./reg
added research-skill v1.4.2 (4 file(s), 4331 bytes) to ./reg

$ agentskillpack registry list --registry ./reg
hello-skill              1.0.0        91805ef91d42a833  3 file(s)
research-skill           1.4.2        efb49156bf085b91  4 file(s)

$ agentskillpack lock examples/research-skill --registry ./reg -o skillpack.lock
locked 1 dependency/ies for 'research-skill' v1.4.2 -> skillpack.lock
```

The resulting `skillpack.lock` (deterministic, pins exact bytes):

```json
{
  "lock_version": 1,
  "root": "research-skill",
  "root_version": "1.4.2",
  "pins": [
    {
      "name": "hello-skill",
      "req": "^1.0",
      "version": "1.0.0",
      "sha256": "91805ef91d42a833133afb742265ab2736d91f7c0dfc04dd6904db26dbd22987"
    }
  ]
}
```

---

## Commands

| Command                          | Synopsis                                              |
|----------------------------------|-------------------------------------------------------|
| `pack <dir> -o <f> [--validate]` | Pack a skill directory into a `.skillpack`            |
| `unpack <f> -o <dir>`            | Unpack (re-verifies each file as it writes)           |
| `verify <f> [--pubkey <k>]`      | Integrity gate; optional signature check (non-zero on failure) |
| `info <f> [--json]`              | Print archive metadata + per-file digests             |
| `manifest validate <dir\|f> [--json]` | Validate a `skill.json` (collects all problems)  |
| `schema`                         | Print the skill JSON Schema                           |
| `keygen -o <dir> [--name <n>]`   | Generate an ed25519 keypair                           |
| `sign <f> --key <priv> [-o <sig>]` | Detached-sign an archive                            |
| `registry add <f> --registry <dir>`   | Install (verifies integrity)                    |
| `registry list --registry <dir>`      | List installed skills                           |
| `registry resolve <name> --req <semver> --registry <dir>` | Resolve name → path (re-verifies) |
| `registry remove <name> --version <v> --registry <dir>`   | Uninstall a version         |
| `lock <dir\|f> --registry <dir> [-o <lock>]` | Write a deterministic `skillpack.lock`    |

---

## Measured results

Measured on this repo's release build (Windows, x86-64, `cargo build
--release`), packing/verifying a synthetic skill of **51 files ≈ 0.98 MB** of
random data. Numbers are wall-clock **per CLI invocation** (they include process
start-up — the number a user or CI step actually experiences), averaged over
repeated runs:

| Operation      | Per invocation | Notes                                       |
|----------------|----------------|---------------------------------------------|
| `pack`         | ≈ 31 ms        | read 51 files, SHA-256 each, write archive  |
| `verify`       | ≈ 16 ms        | re-hash all 51 blobs + structural checks    |
| effective verify throughput | ≈ 60 MB/s | including process spawn + full re-hash |

The container format itself is a single sequential pass with no seeking; cost is
dominated by SHA-256 over the file bytes. Reproduce with your own skill:

```sh
cargo build --release
target/release/agentskillpack pack <your-skill> -o /tmp/s.skillpack
time target/release/agentskillpack verify /tmp/s.skillpack
```

The full test suite (**56 tests**: unit + CLI end-to-end + schema-sync +
round-trip) runs in well under a second.

---

## Demos

Every demo exits 0 and is runnable on any platform:

```sh
demos/run_all.sh        # bash (macOS / Linux / Git Bash)
pwsh demos/run_all.ps1  # PowerShell (Windows / cross-platform pwsh)
```

They cover the full lifecycle (pack → sign → verify → registry add → resolve →
lock → unpack), tamper detection, wrong-key rejection, and manifest validation.

---

## Documentation

- [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) — container format spec, manifest
  model, resolver algorithm, signing/trust model, registry layout.
- [docs/skill.schema.json](docs/skill.schema.json) — the manifest JSON Schema
  (2020-12), generated from the Rust model and kept in sync by a test.
- [docs/authoring.md](docs/authoring.md) — authoring a skill.
- [docs/capabilities.md](docs/capabilities.md) — capabilities & host policy.
- [docs/signing.md](docs/signing.md) — signing workflow & threats defeated.
- [docs/registry.md](docs/registry.md) — registry usage.
- [docs/ci-verification.md](docs/ci-verification.md) — wiring verify into CI.

---

## The `.skillpack` format (at a glance)

All multi-byte integers are **big-endian, unsigned** (portable across
platforms):

```text
+--------------------------------------------------------------+
| MAGIC        | 8 bytes  | ASCII "SKILLPAK"                    |
| FORMAT_VER   | 2 bytes  | u16, currently 1                    |
| HEADER_LEN   | 4 bytes  | u32, byte length of HEADER_JSON     |
| HEADER_JSON  | N bytes  | UTF-8 JSON object (see below)       |
+--------------------------------------------------------------+
| repeated once per file, in HEADER_JSON.files order:          |
|   BLOB_LEN   | 8 bytes  | u64, byte length of this file       |
|   BLOB_DATA  | L bytes  | raw file contents                   |
+--------------------------------------------------------------+
```

Full spec, header schema, and the integrity/versioning model are in
[docs/ARCHITECTURE.md](docs/ARCHITECTURE.md).

---

## Testing

```sh
cargo test                              # 56 tests
cargo clippy --all-targets -- -D warnings
cargo fmt --all -- --check
```

## License

License: COCL 1.0. See [LICENSE](LICENSE) and [DISCLAIMER.md](DISCLAIMER.md).
