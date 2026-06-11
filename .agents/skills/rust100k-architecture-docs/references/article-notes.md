# Article Notes

Source: https://matklad.github.io/2021/02/06/ARCHITECTURE.md.html

Matklad's durable points:

- An architecture document exists to transfer the maintainer's mental map.
- Keep it short and stable; revisit periodically rather than syncing every detail with code.
- Start with a bird's-eye problem overview, then provide a codemap that answers "where is X?" and "what does this thing do?"
- Name important files, modules, and types without creating links that go stale.
- Call out invariants, especially absences that cannot be inferred locally.
- Describe boundaries and cross-cutting concerns.
- Use the doc as a chance to notice when source layout and conceptual layout have drifted.

Mite adaptation:

- `docs/architecture.md` is already the right home.
- The doc should stay focused on `watch`, capture, OCR, smoothing, dictionary/hover, Win32 overlay, latency, and runtime fallback boundaries.
- Detailed latency evidence belongs in `docs/performance.md`; detailed model tradeoffs belong in `docs/models.md` and `docs/accuracy.md`.
