// The Mite mark: an OCR focus-reticle, corner brackets framing a detected line
// with an accent tick for the moment of recognition. Ported from the site's
// MiteMark.astro so the app and site share one brand glyph.

interface MiteMarkProps {
  size?: string;
  title?: string;
  className?: string;
}

export function MiteMark({ size = "1em", title = "Mite", className }: MiteMarkProps) {
  return (
    <svg
      className={className}
      width={size}
      height={size}
      viewBox="0 0 24 24"
      fill="none"
      role="img"
      aria-label={title}
      style={{ flex: "none" }}
      xmlns="http://www.w3.org/2000/svg"
    >
      <g
        stroke="currentColor"
        strokeWidth="1.7"
        strokeLinecap="round"
        strokeLinejoin="round"
      >
        <path d="M3 7.5V4.5C3 3.67 3.67 3 4.5 3H7.5" />
        <path d="M16.5 3H19.5C20.33 3 21 3.67 21 4.5V7.5" />
        <path d="M21 16.5V19.5C21 20.33 20.33 21 19.5 21H16.5" />
        <path d="M7.5 21H4.5C3.67 21 3 20.33 3 19.5V16.5" />
      </g>
      <rect x="7" y="11" width="10" height="2" rx="1" fill="currentColor" opacity="0.5" />
      <rect x="7" y="11" width="4" height="2" rx="1" fill="var(--color-accent)" />
    </svg>
  );
}
