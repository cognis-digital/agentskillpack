# hello-skill

A tiny example skill used to demonstrate the `.skillpack` format.

## What it does

Prints a friendly greeting. The skill is described by `skill.json` and its
behavior lives in `scripts/greet.py`.

## Layout

```
hello-skill/
  skill.json        # manifest: name, version, description, entrypoint
  SKILL.md          # this file
  scripts/
    greet.py        # the skill's logic
```

## Pack it

```sh
agentskillpack pack examples/hello-skill -o hello-skill.skillpack
agentskillpack info hello-skill.skillpack
```
