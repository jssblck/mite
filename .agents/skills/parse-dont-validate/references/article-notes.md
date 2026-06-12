# Article Notes

Source: Alexis King, "Parse, don't validate" (2019-11-05), https://lexi-lambda.github.io/blog/2019/11/05/parse-don-t-validate/

## Core Learning

- Validation checks a weak value and returns no useful value, often `Result<()>`.
- Parsing consumes a weak value and returns a stronger value that preserves the learned fact in the type system.
- Strengthen argument types instead of weakening return types when a precondition can be expressed in data.
- Push proof upward to the boundary where the data is created or received, but no further.
- Avoid shotgun parsing: do not mix input checks through processing code after acting on the input.
- Abstract newtypes with private constructors are acceptable when Rust cannot express the invariant directly, such as numeric ranges.
- Functions returning `m ()` or `Result<()>` deserve suspicion when their main purpose is to reject invalid input.

## Rust Translation

Prefer:

```rust
pub fn parse_eval_spec(spec: EvalSpec) -> Result<CheckedEvalSpec>;
pub fn score_ocr_lookup(spec: &CheckedEvalSpec, result: &OcrLookupResult) -> EvalReport;
```

Avoid:

```rust
pub fn validate_eval_spec(spec: &EvalSpec) -> Result<()>;
pub fn score_ocr_lookup(spec: &EvalSpec, result: &OcrLookupResult) -> EvalReport;
```

The first shape makes parsing mandatory for scoring. The second shape allows a caller to forget validation and still compile.

## Mite Examples

- Config TOML should parse into `AppConfig` with a checked pipeline field, not deserialize and then discard a `validate()` result.
- Eval label JSON should parse into `CheckedEvalSpec` before scoring or corpus aggregation.
- Drafting/editing tools may manipulate raw serde structs while the human is editing, but save/score paths must parse first.
- Build-time or checked-in data should use construction helpers that fail early and centrally, not repeated `expect("valid")` calls throughout runtime code.
