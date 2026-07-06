# Authoring a skill

A *skill* is a directory of files described by a `skill.json` manifest. This
guide walks through creating one from scratch.

## 1. Lay out the directory

```
my-skill/
  skill.json          # the manifest (required)
  SKILL.md            # human docs (recommended)
  scripts/
    run.py            # your entrypoint
  templates/          # any static assets
```

Every regular file under the directory is packed recursively; symlinks are
skipped. Paths are stored forward-slash normalized.

## 2. Write the manifest

Minimum viable manifest:

```json
{
  "name": "my-skill",
  "version": "0.1.0"
}
```

`name` must match `[a-z0-9._-]+` (lowercase, no leading/trailing separator).
`version` must be valid [semver](https://semver.org). A fuller manifest:

```json
{
  "manifest_version": 1,
  "name": "my-skill",
  "version": "0.1.0",
  "description": "One line on what it does.",
  "license": "COCL-1.0",
  "authors": ["You <you@example.com>"],
  "entrypoint": "scripts/run.py",
  "tags": ["example"],
  "capabilities": [
    { "kind": "fs.read", "scope": ["./templates"] }
  ],
  "dependencies": [
    { "name": "hello-skill", "req": "^1.0" }
  ],
  "inputs": [
    { "name": "query", "type": "string", "required": true }
  ],
  "outputs": [
    { "name": "result", "type": "file" }
  ],
  "compat": { "engine": ">=0.3, <2.0" }
}
```

Declare only the capabilities you actually need — see
[capabilities.md](capabilities.md). Under-declaring is safest for the host;
over-declaring erodes the value of the declaration.

## 3. Validate before you pack

```sh
agentskillpack manifest validate my-skill
```

Validation collects *all* problems at once (bad semver, unknown fields, bad
capability scopes, self-dependencies, unsafe entrypoint paths, unknown I/O
types) so you can fix them in one pass. Use `--json` for machine output.

## 4. Pack

```sh
agentskillpack pack my-skill -o my-skill.skillpack --validate
```

`--validate` runs the full manifest check first and refuses to pack an invalid
skill. Inspect the result:

```sh
agentskillpack info my-skill.skillpack
agentskillpack verify my-skill.skillpack
```

## 5. Ship it

Sign it ([signing.md](signing.md)), add it to a registry
([registry.md](registry.md)), and wire `verify` into CI
([ci-verification.md](ci-verification.md)).
