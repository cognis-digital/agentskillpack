# research-skill

A richer example skill demonstrating the full manifest surface: declared
capabilities, a dependency on `hello-skill`, typed inputs/outputs, and engine
compatibility constraints.

## What it declares

| Field          | Value                                                    |
|----------------|----------------------------------------------------------|
| capabilities   | `net` (https), `fs.write` (./reports), `fs.read` (./templates) |
| dependencies   | `hello-skill ^1.0`                                        |
| inputs         | `url` (string, required), `max_words` (number)           |
| outputs        | `report` (file)                                          |
| compat.engine  | `>=0.3, <2.0`                                            |

A host reading this manifest knows — *before running anything* — that the skill
intends to reach the network over https and write under `./reports`. A policy
engine can allow, deny, or prompt on that basis. Nothing here grants the
capability; it only declares intent.

## Layout

```
research-skill/
  skill.json          # rich manifest (validate with: agentskillpack manifest validate)
  SKILL.md            # this file
  templates/
    report.md.tmpl    # a read-only asset (declared via fs.read scope)
  scripts/
    research.py       # entrypoint
```

## Try it

```sh
agentskillpack manifest validate examples/research-skill
agentskillpack pack examples/research-skill -o research.skillpack --validate
agentskillpack info research.skillpack
```
