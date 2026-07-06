# Capabilities and host policy

A capability is a **declaration of intent**. It states what a skill expects to
do — not what it is allowed to do. Nothing in a `.skillpack` grants access to
anything; the value is *transparency*, so a host can decide.

## The capability kinds

| Kind       | Declares the skill will…             | Example scope             |
|------------|--------------------------------------|---------------------------|
| `fs.read`  | read files                           | `["./templates"]`         |
| `fs.write` | create or write files                | `["./reports"]`           |
| `net`      | open network connections             | `["api.example.com:443"]` |
| `exec`     | spawn subprocesses                   | `["git"]`                 |
| `env`      | read environment variables           | `["HOME", "PATH"]`        |

Each capability may carry a `scope` list narrowing it (paths, hosts, program
names, variable names). An empty scope means "unscoped / broad". Scopes are
advisory hints to the host policy engine; agentskillpack does not enforce them
itself.

## How a host uses them

The manifest lets a host implement a policy **before** loading skill code:

```text
skill declares:  net (api.example.com:443), fs.write (./reports)
host policy:     net -> PROMPT, fs.write -> ALLOW under sandbox root, exec -> DENY
decision:        prompt the operator for the one network host; grant a scoped
                 writable dir; the skill declared no exec/env so the sandbox can
                 hard-deny both with confidence.
```

Because absent capabilities are an assertion the skill will not use them, a
sandbox can *default-deny* everything not declared. A skill that later tries to
`exec` when it declared no `exec` capability is doing something its manifest
promised it would not — a signal the host can treat as a violation.

## Authoring guidance

- **Declare the minimum.** Every capability you list is a power the host must
  reason about. Fewer, tightly-scoped capabilities make a skill easier to trust.
- **Scope tightly.** `fs.write: ["./reports"]` is far more trustable than an
  unscoped `fs.write`.
- **Keep it honest.** The declaration is only useful if it matches reality. A
  future host that enforces capabilities will penalize skills that under-declare.

## What this is not

This is not a sandbox and not a security boundary on its own. It is the
*input* to a host's allow/deny decision — the transparency layer that unsigned,
undocumented loose files never provide. Enforcement (seccomp, containers,
language sandboxes, capability-based runtimes) is the host's job; the manifest
tells the host what to enforce.
