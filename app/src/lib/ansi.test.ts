import { describe, expect, it } from "vitest";
import {
  defaultStyle,
  isDefaultStyle,
  parseAnsi,
  xterm256ToRgb,
  type AnsiStyle,
} from "./ansi";

function styled(overrides: Partial<AnsiStyle>): AnsiStyle {
  return { ...defaultStyle(), ...overrides };
}

describe("parseAnsi", () => {
  it("returns a single default span for plain text", () => {
    expect(parseAnsi("model warmup complete")).toEqual([
      { text: "model warmup complete", style: defaultStyle() },
    ]);
  });

  it("returns no spans for an empty line", () => {
    expect(parseAnsi("")).toEqual([]);
  });

  it("splits a tracing-formatted line into styled spans", () => {
    // The exact shape tracing-subscriber's fmt layer emits: dim timestamp,
    // colored level, dim target, then the unstyled message.
    const line =
      "\u001b[2m2026-07-14T00:33:54.845834Z\u001b[0m \u001b[33m WARN\u001b[0m " +
      "\u001b[2mort::logging\u001b[0m\u001b[2m:\u001b[0m timing cache miss";
    expect(parseAnsi(line)).toEqual([
      { text: "2026-07-14T00:33:54.845834Z", style: styled({ dim: true }) },
      { text: " ", style: defaultStyle() },
      { text: " WARN", style: styled({ fg: { kind: "palette", index: 3 } }) },
      { text: " ", style: defaultStyle() },
      { text: "ort::logging", style: styled({ dim: true }) },
      { text: ":", style: styled({ dim: true }) },
      { text: " timing cache miss", style: defaultStyle() },
    ]);
  });

  it("treats an empty SGR parameter list as a reset", () => {
    expect(parseAnsi("\u001b[31mred\u001b[mplain")).toEqual([
      { text: "red", style: styled({ fg: { kind: "palette", index: 1 } }) },
      { text: "plain", style: defaultStyle() },
    ]);
  });

  it("accumulates attributes and clears intensity with SGR 22", () => {
    const spans = parseAnsi("\u001b[1;31mbold red\u001b[22mstill red");
    expect(spans).toEqual([
      {
        text: "bold red",
        style: styled({ bold: true, fg: { kind: "palette", index: 1 } }),
      },
      { text: "still red", style: styled({ fg: { kind: "palette", index: 1 } }) },
    ]);
  });

  it("maps bright foregrounds to palette 8-15", () => {
    expect(parseAnsi("\u001b[90mfaint\u001b[97mbright")).toEqual([
      { text: "faint", style: styled({ fg: { kind: "palette", index: 8 } }) },
      { text: "bright", style: styled({ fg: { kind: "palette", index: 15 } }) },
    ]);
  });

  it("keeps low 256-color indexes symbolic and resolves the rest to rgb", () => {
    expect(parseAnsi("\u001b[38;5;9mx")[0].style.fg).toEqual({
      kind: "palette",
      index: 9,
    });
    expect(parseAnsi("\u001b[38;5;196mx")[0].style.fg).toEqual({
      kind: "rgb",
      r: 255,
      g: 0,
      b: 0,
    });
    expect(parseAnsi("\u001b[38;5;232mx")[0].style.fg).toEqual({
      kind: "rgb",
      r: 8,
      g: 8,
      b: 8,
    });
  });

  it("parses truecolor foregrounds and backgrounds", () => {
    const spans = parseAnsi("\u001b[38;2;12;200;77;48;2;300;0;0mx");
    expect(spans[0].style.fg).toEqual({ kind: "rgb", r: 12, g: 200, b: 77 });
    // Out-of-range components clamp instead of producing an invalid color.
    expect(spans[0].style.bg).toEqual({ kind: "rgb", r: 255, g: 0, b: 0 });
  });

  it("accepts the ITU colon-separated color form", () => {
    expect(parseAnsi("\u001b[38:5:208mx")[0].style.fg).toEqual({
      kind: "rgb",
      ...xterm256ToRgb(208),
    });
  });

  it("swallows a malformed extended-color selector without misreading it", () => {
    // "38" with a bogus mode must not leave "9" to be parsed as strikethrough.
    const spans = parseAnsi("\u001b[38;9mx");
    expect(spans[0].style.fg).toBeNull();
    expect(spans[0].style.strikethrough).toBe(false);
  });

  it("sets and clears backgrounds", () => {
    const spans = parseAnsi("\u001b[41mred bg\u001b[49mno bg");
    expect(spans[0].style.bg).toEqual({ kind: "palette", index: 1 });
    expect(spans[1].style.bg).toBeNull();
  });

  it("drops non-SGR escape sequences", () => {
    expect(parseAnsi("\u001b[2Kcleared \u001b[1;5Hmoved \u001b]0;title\u0007done")).toEqual([
      { text: "cleared moved done", style: defaultStyle() },
    ]);
  });

  it("drops an OSC sequence terminated by ESC backslash", () => {
    expect(parseAnsi("\u001b]8;;https://example.test\u001b\\link")).toEqual([
      { text: "link", style: defaultStyle() },
    ]);
  });

  it("drops a truncated escape at end of line", () => {
    expect(parseAnsi("done\u001b[3")).toEqual([
      { text: "done", style: defaultStyle() },
    ]);
  });

  it("drops carriage returns left over from Windows line endings", () => {
    expect(parseAnsi("done\r")).toEqual([{ text: "done", style: defaultStyle() }]);
  });
});

describe("xterm256ToRgb", () => {
  it("maps the color cube corners", () => {
    expect(xterm256ToRgb(16)).toEqual({ r: 0, g: 0, b: 0 });
    expect(xterm256ToRgb(231)).toEqual({ r: 255, g: 255, b: 255 });
    expect(xterm256ToRgb(196)).toEqual({ r: 255, g: 0, b: 0 });
    expect(xterm256ToRgb(46)).toEqual({ r: 0, g: 255, b: 0 });
    expect(xterm256ToRgb(21)).toEqual({ r: 0, g: 0, b: 255 });
  });

  it("maps the grayscale ramp", () => {
    expect(xterm256ToRgb(232)).toEqual({ r: 8, g: 8, b: 8 });
    expect(xterm256ToRgb(255)).toEqual({ r: 238, g: 238, b: 238 });
  });
});

describe("isDefaultStyle", () => {
  it("is true only for the untouched default", () => {
    expect(isDefaultStyle(defaultStyle())).toBe(true);
    expect(isDefaultStyle(styled({ dim: true }))).toBe(false);
    expect(isDefaultStyle(styled({ fg: { kind: "palette", index: 3 } }))).toBe(false);
  });
});
