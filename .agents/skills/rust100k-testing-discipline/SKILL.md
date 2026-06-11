---
name: rust100k-testing-discipline
description: Apply Matklad Rust100k testing discipline to Mite. Use when adding, reviewing, or reorganizing Rust tests, choosing unit versus integration tests, designing fixture formats, deciding whether to mock, adding doctests, or changing scripts/precommit.ps1 and cargo test coverage.
---

# Rust100k Testing Discipline

Design tests around product behavior and data, keep them fast, and avoid first-party mocks. This skill combines Matklad's `Delete Cargo Integration Tests` and `How to Test` guidance with the Mite conflict decisions in `.agents/skills/readme.md`.

## Workflow

1. Define the feature boundary first. For Mite, good boundaries are OCR lookup output, dictionary segmentation, hover geometry, config parsing, and eval scoring.
2. Prefer a small `check(...)` helper with data inputs and expected data outputs over many tests that call internal APIs directly.
3. Keep core tests sans IO: build values in memory and let the function under test compute.
4. Use externalized fixture files when they make cases easy to add, but keep at least one small smoke test that can be run/debugged directly from the IDE.
5. For internal Mite code, prefer unit tests in `src/` over Cargo integration crates. If a separate integration crate is needed, use one modular crate, not many root `tests/*.rs` binaries.
6. Do not mock first-party code. Use real pure functions, in-memory data, deterministic fixtures, or service-level doubles such as localstack for external services.
7. Keep real executable doctests. Mite is not large enough to disable them for build-time reasons.
8. Run `nudge check Cargo.toml src docs examples build.rs` and `cargo test`.

## Mite Policy

- Matklad wins on test placement and first-party mocking.
- `rust-skills` wins on executable doctests for this project size.
- Inline `#[cfg(test)] mod tests { ... }` blocks already exist. When touching a large test module, prefer migrating that module to `#[cfg(test)] mod tests;` plus a sibling `tests.rs`.
- The deterministic OCR fixture engine is allowed for UI smoke work because it is fixture data generation, not a first-party interaction mock.
- Avoid sleep-based synchronization in tests. If concurrency is involved, expose a join, receiver, or observable side channel.

## Validation

```powershell
nudge check Cargo.toml src docs examples build.rs
cargo test
```

Nudge enforces the no-mockall, no first-party `Mock*` identifiers, doctests-on, no Cargo integration test crates, and no sleep-based root integration tests rules. `scripts/precommit.ps1` includes `tests` automatically when that directory exists. Inline test-module migration remains a review rule for touched files because the current repo has existing inline tests.

Read `references/article-notes.md` for source notes and conflict context.
