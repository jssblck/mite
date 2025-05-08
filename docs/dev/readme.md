
# development

Tags denote releases.
Any commit merged to `main` is expected to be release ready,
with the exception of the `version` in `Cargo.toml`.
For more detail, see the [release process](#release-process).

Follows [semver](https://semver.org/):
- MAJOR version indicates a user facing breaking change.
- MINOR version indicates backwards compatible functionality improvement.
- PATCH version indicates backwards compatible bug fixes.

The initial beta releases use `0` as the major version; when this changes to `1`
it will not necessarily indicate a breaking change, but future major version increases will.

## compatibility

- Tracks the latest version of the Rust compiler and associated tooling at all times.
- Tracks the latest Rust language edition.
- Aggressively upgrades dependencies. Relies on testing to validate dependencies work.

## setting up your development environment

I recommend Visual Studio Code or Zed, with the `rust-analyzer` extension.
Install Rust here: https://www.rust-lang.org/tools/install

These tools may be useful, although they're not required:
```
cargo nextest # https://nexte.st/
cargo upgrade # https://lib.rs/crates/cargo-upgrades
cargo machete # https://lib.rs/crates/cargo-machete
```

## style guide

Make your code look like the code around it. Consistency is the name of the game.

You should submit changes to this doc if you think you can improve it,
or if a case should be covered by this doc, but currently is not.

Use `rustfmt` for formatting.
CI enforces that all changes pass a `rustfmt` run with no differences.
CI ensures that all patches pass `clippy` checks.

Comments should describe the "why", type signatures should describe the "what", and the code should describe the "how".

We use the [Rust API Guidelines](https://rust-lang.github.io/api-guidelines/about.html)
during code review; if you want to get ahead of the curve check it out!

Ideally, every PR should check for updated dependencies and update them if applicable;
if this is not realistic at minimum every non-bugfix release **must** ensure dependencies are up to date.

The `.cursor/rules/conventions` file also describes more specific code conventions for Cursor and other
compatible agents; but keep in mind _these are optional for humans_ even though they are _recommended_.

## releasing

Releasing consists of simply pushing a new tag in the format `vX.Y.Z`;
CI automatically builds a new release with generated changelogs.
