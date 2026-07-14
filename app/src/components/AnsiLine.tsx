import { memo, type CSSProperties } from "react";
import { parseAnsi, isDefaultStyle, type AnsiColor, type AnsiStyle } from "../lib/ansi";

// Renders one engine-log line with its ANSI styling as real colored spans.
// The 16-color palette resolves through the .log-view --ansi-N custom
// properties so the terminal hues stay inside the app's design tokens;
// 256-index and truecolor escapes arrive as exact RGB and render inline.

function colorValue(color: AnsiColor): string {
  return color.kind === "palette"
    ? `var(--ansi-${color.index})`
    : `rgb(${color.r} ${color.g} ${color.b})`;
}

function spanClass(style: AnsiStyle): string | undefined {
  const classes: string[] = [];
  if (style.bold) classes.push("ansi-bold");
  if (style.dim) classes.push("ansi-dim");
  if (style.italic) classes.push("ansi-italic");
  if (style.underline) classes.push("ansi-underline");
  if (style.strikethrough) classes.push("ansi-strike");
  return classes.length > 0 ? classes.join(" ") : undefined;
}

function spanStyle(style: AnsiStyle): CSSProperties | undefined {
  const css: CSSProperties = {};
  if (style.fg) css.color = colorValue(style.fg);
  if (style.bg) css.backgroundColor = colorValue(style.bg);
  return style.fg || style.bg ? css : undefined;
}

/**
 * Memoized because the log view re-renders on every appended line while the
 * existing lines never change, so their parses should not be repeated.
 */
export const AnsiLine = memo(function AnsiLine({ text }: { text: string }) {
  const spans = parseAnsi(text);
  // The common unstyled line renders as a bare text node, no span wrappers.
  if (spans.length === 1 && isDefaultStyle(spans[0].style)) {
    return spans[0].text;
  }
  return (
    <>
      {spans.map((span, index) =>
        isDefaultStyle(span.style) ? (
          span.text
        ) : (
          <span key={index} className={spanClass(span.style)} style={spanStyle(span.style)}>
            {span.text}
          </span>
        ),
      )}
    </>
  );
});
