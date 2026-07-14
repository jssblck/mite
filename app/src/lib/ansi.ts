// Parser for the ANSI SGR (color/style) escape sequences carried by the mite
// CLI's tracing output, so the engine log panel can render the intended colors
// instead of raw escape bytes. Pure and DOM-free so it unit-tests without a
// browser; rendering lives in components/AnsiLine.tsx.

/**
 * A resolved text color. The classic 16-entry palette stays symbolic so the
 * renderer can map it onto the app's design tokens; 256-index and truecolor
 * sequences are resolved to concrete RGB at parse time because they already
 * name an exact color.
 */
export type AnsiColor =
  | { kind: "palette"; index: number }
  | { kind: "rgb"; r: number; g: number; b: number };

export interface AnsiStyle {
  bold: boolean;
  dim: boolean;
  italic: boolean;
  underline: boolean;
  strikethrough: boolean;
  fg: AnsiColor | null;
  bg: AnsiColor | null;
}

/** A run of text sharing one style. */
export interface AnsiSpan {
  text: string;
  style: AnsiStyle;
}

export function defaultStyle(): AnsiStyle {
  return {
    bold: false,
    dim: false,
    italic: false,
    underline: false,
    strikethrough: false,
    fg: null,
    bg: null,
  };
}

/** True when the style would render identically to unstyled text. */
export function isDefaultStyle(style: AnsiStyle): boolean {
  return (
    !style.bold &&
    !style.dim &&
    !style.italic &&
    !style.underline &&
    !style.strikethrough &&
    style.fg === null &&
    style.bg === null
  );
}

/**
 * Resolve an xterm 256-color index to RGB: 0-15 are the classic palette
 * (returned as-is by the caller, never passed here), 16-231 a 6x6x6 color
 * cube, 232-255 a 24-step grayscale ramp.
 */
export function xterm256ToRgb(index: number): { r: number; g: number; b: number } {
  if (index >= 232) {
    const v = 8 + (index - 232) * 10;
    return { r: v, g: v, b: v };
  }
  const cube = index - 16;
  const steps = [0, 95, 135, 175, 215, 255];
  return {
    r: steps[Math.floor(cube / 36) % 6],
    g: steps[Math.floor(cube / 6) % 6],
    b: steps[cube % 6],
  };
}

function clampByte(value: number): number {
  return Math.min(255, Math.max(0, Math.trunc(value)));
}

/** Parse one color argument: index 0-255 for `5`-form, RGB for `2`-form. */
function extendedColor(params: number[], at: number): { color: AnsiColor | null; consumed: number } {
  if (params[at] === 5 && at + 1 < params.length) {
    const index = clampByte(params[at + 1]);
    const color: AnsiColor =
      index < 16 ? { kind: "palette", index } : { kind: "rgb", ...xterm256ToRgb(index) };
    return { color, consumed: 2 };
  }
  if (params[at] === 2 && at + 3 < params.length) {
    return {
      color: {
        kind: "rgb",
        r: clampByte(params[at + 1]),
        g: clampByte(params[at + 2]),
        b: clampByte(params[at + 3]),
      },
      consumed: 4,
    };
  }
  // Malformed or truncated color spec: swallow the selector so the remaining
  // arguments are not misread as independent SGR codes.
  return { color: null, consumed: params.length - at };
}

/** Apply one SGR parameter string (the bytes between `ESC [` and `m`). */
function applySgr(style: AnsiStyle, raw: string): AnsiStyle {
  const next = { ...style };
  // ITU T.416 separates color arguments with colons; nu-ansi-term (what
  // tracing uses) emits semicolons. Normalizing keeps both forms working.
  const params = raw.replace(/:/g, ";").split(";").map((p) => (p === "" ? 0 : Number(p)));
  let i = 0;
  while (i < params.length) {
    const code = params[i];
    switch (code) {
      case 0:
        Object.assign(next, defaultStyle());
        break;
      case 1:
        next.bold = true;
        break;
      case 2:
        next.dim = true;
        break;
      case 3:
        next.italic = true;
        break;
      case 4:
        next.underline = true;
        break;
      case 9:
        next.strikethrough = true;
        break;
      case 22:
        next.bold = false;
        next.dim = false;
        break;
      case 23:
        next.italic = false;
        break;
      case 24:
        next.underline = false;
        break;
      case 29:
        next.strikethrough = false;
        break;
      case 39:
        next.fg = null;
        break;
      case 49:
        next.bg = null;
        break;
      case 38:
      case 48: {
        const { color, consumed } = extendedColor(params, i + 1);
        if (code === 38) next.fg = color ?? next.fg;
        else next.bg = color ?? next.bg;
        i += consumed;
        break;
      }
      default:
        if (code >= 30 && code <= 37) next.fg = { kind: "palette", index: code - 30 };
        else if (code >= 90 && code <= 97) next.fg = { kind: "palette", index: code - 82 };
        else if (code >= 40 && code <= 47) next.bg = { kind: "palette", index: code - 40 };
        else if (code >= 100 && code <= 107) next.bg = { kind: "palette", index: code - 92 };
        // Anything else (blink, reverse, fonts) has no sensible rendering in
        // a log panel and is ignored.
        break;
    }
    i++;
  }
  return next;
}

const ESC = "\u001b";
const BEL = "\u0007";

/**
 * Split a log line into styled spans. SGR sequences update the running style;
 * every other escape sequence (cursor movement, OSC titles, etc.) is dropped,
 * matching what a terminal would visibly show. Each line starts from the
 * default style: tracing resets after every styled token, so no state needs
 * to carry across lines.
 */
export function parseAnsi(line: string): AnsiSpan[] {
  const spans: AnsiSpan[] = [];
  let style = defaultStyle();
  let text = "";

  const flush = () => {
    if (text) {
      spans.push({ text, style });
      text = "";
    }
  };

  let i = 0;
  while (i < line.length) {
    const ch = line[i];
    if (ch !== ESC) {
      // Stray carriage returns (Windows line endings surviving the reader)
      // would render as visible boxes in the DOM; drop them.
      if (ch !== "\r") text += ch;
      i++;
      continue;
    }
    const kind = line[i + 1];
    if (kind === "[") {
      // CSI: parameter and intermediate bytes run to the first final byte in
      // 0x40-0x7e. A truncated sequence at end-of-line is silently dropped.
      let j = i + 2;
      while (j < line.length && !(line.charCodeAt(j) >= 0x40 && line.charCodeAt(j) <= 0x7e)) {
        j++;
      }
      if (j >= line.length) break;
      if (line[j] === "m") {
        flush();
        style = applySgr(style, line.slice(i + 2, j));
      }
      i = j + 1;
    } else if (kind === "]") {
      // OSC: runs to BEL or the ESC \ string terminator.
      let j = i + 2;
      while (j < line.length && line[j] !== BEL && !(line[j] === ESC && line[j + 1] === "\\")) {
        j++;
      }
      i = j >= line.length ? line.length : line[j] === BEL ? j + 1 : j + 2;
    } else {
      // Two-byte escape (RIS, charset selection, ...): drop both bytes.
      i += 2;
    }
  }
  flush();
  return spans;
}
