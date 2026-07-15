import * as React from "react";

// points in an X/Y plane (latency vs cost, p50 vs p95) with an interactive
// hover tooltip. ported from the Rolter Design System
// components/charts/ScatterPlot.jsx
export interface ScatterPoint {
  x: number;
  y: number;
  r?: number;
  color?: string;
  group?: number;
  label?: string;
}

export interface ScatterPlotProps extends React.HTMLAttributes<HTMLDivElement> {
  points: ScatterPoint[];
  groups?: string[];
  xMax?: number;
  yMax?: number;
  xLabel?: string;
  yLabel?: string;
  xUnit?: string;
  yUnit?: string;
  height?: number;
  color?: string;
}

const PALETTE = [
  "var(--zinc-300)",
  "var(--red-folk)",
  "var(--status-info)",
  "var(--status-success)",
  "var(--status-warning)",
];

function fmt(v: number): string | number {
  if (v >= 1000) return (v / 1000).toFixed(v % 1000 === 0 ? 0 : 1) + "k";
  return Math.round(v * 100) / 100;
}

interface HoverState {
  i: number;
  x: number;
  y: number;
  p: ScatterPoint;
  c: string;
}

const legendStyle: React.CSSProperties = {
  display: "flex",
  gap: "var(--space-4, 0.75rem)",
  flexWrap: "wrap",
  marginTop: "var(--space-2, 0.375rem)",
};
const legendItem: React.CSSProperties = {
  display: "inline-flex",
  alignItems: "center",
  gap: 6,
  fontSize: "var(--text-xs)",
  color: "var(--text-muted)",
};

export function ScatterPlot({
  points = [],
  groups,
  xMax,
  yMax,
  xLabel = "",
  yLabel = "",
  xUnit = "",
  yUnit = "",
  height = 220,
  color = "var(--zinc-300)",
  className,
  ...props
}: ScatterPlotProps) {
  const [hover, setHover] = React.useState<HoverState | null>(null);
  const W = 640;
  const padL = 48;
  const padR = 14;
  const padT = 14;
  const padB = 34;
  const H = height;
  const iw = W - padL - padR;
  const ih = H - padT - padB;
  const all = points.length ? points : [];
  const xm = xMax ?? (Math.max(...all.map((p) => p.x), 0) * 1.1 || 1);
  const ym = yMax ?? (Math.max(...all.map((p) => p.y), 0) * 1.1 || 1);
  const xAt = (v: number) => padL + (v / xm) * iw;
  const yAt = (v: number) => padT + ih - (v / ym) * ih;
  const ticks = 4;

  return (
    <div className={className} style={{ position: "relative" }} {...props}>
      <svg
        viewBox={`0 0 ${W} ${H}`}
        width="100%"
        height={H}
        role="img"
        preserveAspectRatio="xMidYMid meet"
        style={{ display: "block", overflow: "visible" }}
      >
        {Array.from({ length: ticks + 1 }).map((_, i) => {
          const y = padT + (ih / ticks) * i;
          const v = (ym / ticks) * (ticks - i);
          return (
            <g key={"y" + i}>
              <line x1={padL} y1={y} x2={W - padR} y2={y} stroke="var(--border-subtle)" strokeWidth="1" />
              <text
                x={padL - 8}
                y={y + 3}
                textAnchor="end"
                fontSize="10"
                fontFamily="var(--font-mono)"
                fill="var(--text-subtle)"
              >
                {fmt(v)}
                {yUnit}
              </text>
            </g>
          );
        })}
        {Array.from({ length: ticks + 1 }).map((_, i) => {
          const x = padL + (iw / ticks) * i;
          const v = (xm / ticks) * i;
          return (
            <text
              key={"x" + i}
              x={x}
              y={H - 18}
              textAnchor="middle"
              fontSize="10"
              fontFamily="var(--font-mono)"
              fill="var(--text-subtle)"
            >
              {fmt(v)}
              {xUnit}
            </text>
          );
        })}
        {points.map((p, i) => {
          const c =
            p.color || (p.group != null && groups ? PALETTE[p.group % PALETTE.length] : color);
          const active = hover && hover.i === i;
          return (
            <g key={i}>
              <circle
                cx={xAt(p.x)}
                cy={yAt(p.y)}
                r={(p.r || 4) + (active ? 2 : 0)}
                fill={c}
                opacity={active ? 1 : 0.85}
                stroke="var(--surface-base)"
                strokeWidth={active ? 2 : 1}
                style={{ cursor: "pointer", transition: "r .1s" }}
                onMouseEnter={() => setHover({ i, x: xAt(p.x), y: yAt(p.y), p, c })}
                onMouseLeave={() => setHover(null)}
              >
                <title>
                  {(p.label != null ? p.label + "  —  " : "") +
                    `(${fmt(p.x)}${xUnit}, ${fmt(p.y)}${yUnit})`}
                </title>
              </circle>
            </g>
          );
        })}
        {xLabel && (
          <text x={padL + iw / 2} y={H - 2} textAnchor="middle" fontSize="10" fill="var(--text-muted)">
            {xLabel}
          </text>
        )}
        {yLabel && (
          <text
            x={12}
            y={padT + ih / 2}
            textAnchor="middle"
            fontSize="10"
            fill="var(--text-muted)"
            transform={`rotate(-90 12 ${padT + ih / 2})`}
          >
            {yLabel}
          </text>
        )}
      </svg>
      {hover && (
        <div
          style={{
            position: "absolute",
            left: `${(hover.x / W) * 100}%`,
            top: `${(hover.y / H) * 100}%`,
            transform: "translate(-50%, calc(-100% - 10px))",
            pointerEvents: "none",
            zIndex: 2,
            background: "var(--surface-elevated)",
            border: "1px solid var(--border-default)",
            borderRadius: "var(--radius-md)",
            boxShadow: "var(--shadow-md)",
            padding: "6px 9px",
            whiteSpace: "nowrap",
          }}
        >
          {hover.p.label != null && (
            <div
              style={{
                display: "flex",
                alignItems: "center",
                gap: 6,
                fontSize: "var(--text-xs)",
                color: "var(--text-primary)",
                fontWeight: 500,
              }}
            >
              <span
                style={{ width: 8, height: 8, borderRadius: "50%", background: hover.c, flex: "none" }}
              />
              {hover.p.label}
            </div>
          )}
          <div
            style={{
              fontFamily: "var(--font-mono)",
              fontSize: "var(--text-2xs)",
              color: "var(--text-muted)",
              marginTop: hover.p.label != null ? 2 : 0,
            }}
          >
            {xLabel || "x"} {fmt(hover.p.x)}
            {xUnit} · {yLabel || "y"} {fmt(hover.p.y)}
            {yUnit}
          </div>
        </div>
      )}
      {groups && groups.length > 0 && (
        <div style={legendStyle}>
          {groups.map((g, i) => (
            <span key={i} style={legendItem}>
              <span
                style={{ width: 8, height: 8, borderRadius: "50%", background: PALETTE[i % PALETTE.length] }}
              />
              {g}
            </span>
          ))}
        </div>
      )}
    </div>
  );
}
