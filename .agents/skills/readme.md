# Repo-Local Rust Skills

This directory contains two Rust guidance sources:

- `rust-skills/`: installed from `leonardomso/rust-skills` at commit `89910e8585331dabbecd400ae132b4070ecf24af`.
- `rust100k-*`: Mite-specific skills derived from Matklad's Rust100k article index at https://matklad.github.io/2021/09/05/Rust100k.html.

## Conflict Decisions

These are the semantic conflicts found while installing the skills and reading the Rust100k series.

| Topic | Matklad | rust-skills | Mite decision |
|---|---|---|---|
| Cargo integration tests | Internal crates should avoid integration crates; public libraries should use at most one modular integration crate. | Put integration tests under `tests/`, with examples using multiple files. | Prefer Matklad. Mite is an internal application, so use `src/` tests by default and at most one modular integration crate if a true external boundary needs it. |
| Test module shape | For larger test bodies, use `#[cfg(test)] mod tests;` and a sibling `tests.rs` so test-only edits avoid normal library recompilation. | Use inline `#[cfg(test)] mod tests { ... }`. | Prefer Matklad. Existing inline tests may remain, but migrate large touched modules to sibling `tests.rs`. |
| Doctests | Disable doctests for internal libraries in large projects when link cost dominates. | Keep examples executable as doctests. | Prefer `rust-skills` for Mite today. This is not yet a large project, and executable docs are valuable. Revisit if doctests become a measurable build-time problem. |
| Mocking | Favor boundary/data-driven tests and observability over mocks. | Use trait mocks and `mockall` for isolation. | Prefer Matklad, strongly. First-party mocking is an antipattern here. Service-level doubles for external services are acceptable; first-party trait mocks and `mockall` are not. |
| Generic and `dyn` boundaries | In large systems, avoid generic code across crate boundaries; use thin ergonomic wrappers over concrete or `dyn` internals. | Prefer `impl Trait` or generics over type erasure for runtime performance. | Prefer `rust-skills` for Mite today. Mite is small enough that runtime clarity/performance usually wins. Revisit if compile-time or monomorphization evidence changes. |

## Enforcement

The repo-level check is:

```powershell
nudge check Cargo.toml src docs examples build.rs
```

`scripts\precommit.ps1` runs that command before `cargo test` and `cargo clippy --all-targets -- -D warnings`, adding `tests` automatically if that directory exists. Plain `nudge check` currently reports zero checked files in this Windows repo, so use the explicit paths above.

Nudge is intentionally strict for high-confidence file-pattern policies:

- New Cargo integration test crates are blocked.
- Doctests are not disabled.
- `mockall`, `#[automock]`, and first-party `Mock*` Rust identifiers are rejected.
- `#[inline(always)]` is rejected unless the policy is deliberately changed after measurement.
- `docs/architecture.md` rejects fragile local Markdown links.

Cargo/Clippy config enforces what fits Rust-native tooling:

- `Cargo.toml` keeps explicit Clippy lint groups.
- `clippy::inline_always` is denied.

Some decisions remain review rules because they require context or absence checks that do not fit pure Nudge/Cargo/Clippy config:

- Inline test modules are reported as warnings. Migrate them to sibling `tests.rs` files when the relevant tests are touched.
- `docs/architecture.md` must keep its required sections and important boundary terms.
- Public generic and `impl Trait` functions are allowed for Mite today, but should be revisited if build-time evidence changes.
