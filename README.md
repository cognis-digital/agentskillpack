# agentskillpack

A portable packaging format and CLI for distributing **AI-agent skills**.

A *skill* is just a directory of files (scripts, prompts, data) described by a
`skill.json` manifest. `agentskillpack` rolls that directory into a single,
self-describing, integrity-checked `.skillpack` archive that you can publish,
copy, and unpack anywhere — with a built-in `verify` you can wire into CI.

- **pack** a skill directory into one `.skillpack` file
- **unpack** it back out, byte-for-byte identical (lossless round-trip)
- **verify** integrity (SHA-256 per file + structural checks) — exits non-zero
  on any mismatch, so it works as a CI gate
- **info** prints the manifest metadata and per-file hashes (with `--json`)

No external services, no network calls, minimal dependencies (`serde`,
`serde_json`, `sha2`).

License: COCL 1.0. Maintainer: Cognis Digital.


<!-- cognis:example:start -->
## 🔎 Example output

**Sample result format** _(illustrative values — run on your own data for real findings):_

```
{
  "agent": {
    "id": "1234",
    "name": "John Doe"
  },
  "skills": [
    {
      "id": "1",
      "name": "Python Programming",
      "level": 8,
      "certified": true
    },
    {
      "id": "2",
      "name": "Data Analysis",
      "level": 6,
      "certified": false
    }
  ]
}
```

<!-- cognis:example:end -->

## Install / build

```sh
cargo build --release
# binary at target/release/agentskillpack
```

## Usage

```sh
# Pack the bundled example skill
agentskillpack pack examples/hello-skill -o hello.skillpack

# Inspect it
agentskillpack info hello.skillpack
agentskillpack info hello.skillpack --json

# Verify integrity (exit code 0 = clean, non-zero = tampered/corrupt)
agentskillpack verify hello.skillpack

# Unpack it elsewhere
agentskillpack unpack hello.skillpack -o ./restored
```

### Commands

| Command  | Synopsis                                    | Notes                                   |
|----------|---------------------------------------------|-----------------------------------------|
| `pack`   | `pack <skill-dir> -o <out.skillpack>`       | Reads `skill.json` for name/version     |
| `unpack` | `unpack <archive> -o <dir>`                 | Re-verifies each file as it writes      |
| `verify` | `verify <archive>`                          | Non-zero exit on integrity failure      |
| `info`   | `info <archive> [--json]`                   | Metadata + per-file digests             |

## Skill directory

A skill directory should contain a `skill.json` manifest. Recognized fields:

```json
{
  "name": "hello-skill",
  "version": "1.0.0",
  "description": "A minimal sample agent skill."
}
```

`name`/`version`/`description` are copied into the archive header. Unknown
fields are ignored (and preserved as ordinary packed files, since `skill.json`
is itself stored in the archive). If `skill.json` is absent, the directory name
is used as the skill name and the version defaults to `0.0.0`. Every regular
file under the directory (recursively) is packed; symlinks are skipped.

## Format spec

A `.skillpack` archive is a single byte stream. All multi-byte integers are
**big-endian, unsigned**. The layout is:

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

`HEADER_JSON` is a JSON object:

```json
{
  "format_version": 1,
  "name": "hello-skill",
  "version": "1.0.0",
  "description": "optional, omitted if absent",
  "files": [
    { "path": "skill.json", "size": 123, "sha256": "<64 hex chars>" },
    { "path": "scripts/greet.py", "size": 456, "sha256": "<64 hex chars>" }
  ]
}
```

The file blobs follow the header in the **exact order** of `files`, so a reader
can stream them out sequentially without seeking. Each `FileEntry` records:

- `path` — relative path within the skill, normalized to forward slashes.
  Absolute paths, drive prefixes, and `..` traversal are rejected on both pack
  and unpack.
- `size` — file length in bytes (must equal the matching `BLOB_LEN`).
- `sha256` — lowercase hex SHA-256 of the file contents.

### Integrity model

`verify` (and `unpack`) recompute the SHA-256 of every blob and compare it to
the recorded `sha256`, and check that each blob's length matches its recorded
`size`. Structural checks also reject bad magic, an unsupported
`format_version`, truncated data, and trailing bytes after the final blob. Any
failure makes `verify` exit non-zero.

### Versioning

The version is recorded twice: once in the binary `FORMAT_VER` field and once
in `HEADER_JSON.format_version`; both must equal the version this build
understands (`1`). A future, incompatible layout will bump this number, letting
old readers refuse new archives cleanly rather than misparse them.

## Testing

```sh
cargo test
```

Unit tests cover hashing (against known SHA-256 vectors), path-traversal
rejection, header serialize/parse round-trips, and tamper detection.
Integration tests (`tests/roundtrip.rs`) pack a real multi-file skill
(including a binary file spanning all 256 byte values), unpack it, and assert
the trees are byte-for-byte equal, plus verify-pass / tamper-fail / truncation
cases.

## License

License: COCL 1.0.
