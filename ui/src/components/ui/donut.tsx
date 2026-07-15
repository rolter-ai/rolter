// proportional share (provider traffic, cost by model) via SVG arcs.
// ported from the Rolter Design System components/charts/Donut.jsx
export interface DonutSegment {
  label: string;
  value: number;
  color?: string;
}

export interface DonutProps extends React.HTMLAttributes<HTMLDivElement> {
  segments: DonutSegment[];
  size?: number;
  thickness?: number;
  centerLabel?: React.ReactNode;
  centerSub?: React.ReactNode;
  legend?: boolean;
  maxSegments?: number;
}

const PALETTE = [
  "var(--red-folk)",
  "var(--zinc-400)",
  "var(--status-info)",
  "var(--status-success)",
  "var(--status-warning)",
  "var(--red-500)",
  "var(--zinc-600)",
  "var(--zinc-300)",
];
const OTHER = "var(--zinc-700)";

export function Donut({
  segments = [],
  size = 168,
  thickness = 24,
  centerLabel,
  centerSub,
  legend = true,
  maxSegments = 6,
  className,
  ...props
}: DonutProps) {
  // roll everything past the top (maxSegments-1) into a single "Other" slice
  // so a donut of 12+ providers stays legible.
  let segs = segments;
  if (maxSegments && segments.length > maxSegments) {
    const sorted = [...segments].sort((a, b) => b.value - a.value);
    const head = sorted.slice(0, maxSegments - 1);
    const rest = sorted.slice(maxSegments - 1);
    const restTotal = rest.reduce((a, s) => a + s.value, 0);
    segs = [...head, { label: `Other (${rest.length})`, value: restTotal, color: OTHER }];
  }
  const total = segs.reduce((a, s) => a + s.value, 0) || 1;
  const r = (size - thickness) / 2;
  const c = size / 2;
  const circ = 2 * Math.PI * r;
  let offset = 0;

  return (
    <div
      className={className}
      style={{ display: "flex", alignItems: "center", gap: "var(--space-6, 1.25rem)" }}
      {...props}
    >
      <svg viewBox={`0 0 ${size} ${size}`} width={size} height={size} style={{ flex: "none" }}>
        <circle cx={c} cy={c} r={r} fill="none" stroke="var(--surface-subtle)" strokeWidth={thickness} />
        {segs.map((s, i) => {
          const frac = s.value / total;
          const dash = frac * circ;
          const el = (
            <circle
              key={i}
              cx={c}
              cy={c}
              r={r}
              fill="none"
              stroke={s.color || PALETTE[i % PALETTE.length]}
              strokeWidth={thickness}
              strokeDasharray={`${dash} ${circ - dash}`}
              strokeDashoffset={-offset}
              transform={`rotate(-90 ${c} ${c})`}
              strokeLinecap="butt"
            />
          );
          offset += dash;
          return el;
        })}
        {centerLabel != null && (
          <text
            x={c}
            y={c - 2}
            textAnchor="middle"
            fontSize="22"
            fontFamily="var(--font-mono)"
            fontWeight="600"
            fill="var(--text-primary)"
          >
            {centerLabel}
          </text>
        )}
        {centerSub != null && (
          <text
            x={c}
            y={c + 16}
            textAnchor="middle"
            fontSize="10"
            fontFamily="var(--font-mono)"
            fill="var(--text-muted)"
          >
            {centerSub}
          </text>
        )}
      </svg>
      {legend && (
        <div style={{ display: "flex", flexDirection: "column", gap: "var(--space-2, 0.375rem)" }}>
          {segs.map((s, i) => (
            <span
              key={i}
              style={{
                display: "inline-flex",
                alignItems: "center",
                gap: 8,
                fontSize: "var(--text-xs)",
                color: "var(--text-secondary)",
              }}
            >
              <span
                style={{
                  width: 10,
                  height: 10,
                  borderRadius: 2,
                  background: s.color || PALETTE[i % PALETTE.length],
                  flex: "none",
                }}
              />
              <span style={{ minWidth: 90 }}>{s.label}</span>
              <span style={{ fontFamily: "var(--font-mono)", color: "var(--text-muted)" }}>
                {Math.round((s.value / total) * 100)}%
              </span>
            </span>
          ))}
        </div>
      )}
    </div>
  );
}
