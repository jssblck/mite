---
name: rust100k-architecture-docs
description: Maintain Rust project architecture documentation in the Matklad Rust100k style. Use when changing Mite module boundaries, runtime layers, cross-cutting concerns, architectural invariants, docs/architecture.md, AGENTS.md architecture maps, or when reviewing whether code structure and architecture docs still agree.
---

# Rust100k Architecture Docs

Keep Mite's architecture docs short, stable, and useful as a map. This skill comes from Matklad's `ARCHITECTURE.md` article in the Rust100k series.

## Workflow

1. Start with `docs/architecture.md`, then inspect the touched Rust modules.
2. Update architecture docs only for durable structure: problem overview, coarse boundaries, invariants, and cross-cutting concerns.
3. Name important modules, files, traits, and types, but avoid fragile Markdown links to local code paths.
4. Call out important absences and boundaries explicitly, such as "Win32-free hover logic stays in hover/*" or "watch does not use a generic pipeline scheduler".
5. Keep implementation details in code comments, module docs, or narrower docs when they are likely to churn.
6. Run `nudge check docs/architecture.md` and the usual cargo checks.

## Mite Policy

- Preserve the first plain-language overview for humans who are new to the project.
- Keep `## Boundaries`, `## Real OCR runtime`, `## Pipeline latency`, and `## Latency acceptance` current when touching capture, OCR, dictionary lookup, smoothing, overlay, or timing.
- If code has moved, update the codemap rather than adding a migration note.
- If a rule is enforced by type construction or config validation, state the invariant where the boundary is described.
- If a topic becomes too detailed, split it into a focused doc and leave only a pointer-level summary in `docs/architecture.md`.

## Validation

Run the structural audit before the usual cargo checks:

```powershell
nudge check docs/architecture.md
```

Nudge blocks fragile local links in `docs/architecture.md`. Required sections and boundary terms are still a human review rule because pure Nudge regex rules cannot detect missing concepts.

Read `references/article-notes.md` for the source summary and source URL.
