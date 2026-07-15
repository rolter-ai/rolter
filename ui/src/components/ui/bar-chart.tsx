// vertical bars (doubles as a histogram) with an optional horizontal ranked
// mode for 10+ categories. ported from the Rolter Design System
// components/charts/BarChart.jsx
export interface BarChartProps extends React.HTMLAttributes<HTMLDivElement> {
  data: number[];
  labels?: (string | number)[];
  height?: number;
  color?: string;
  highlightColor?: string;
  highlightLast?: boolean;
  horizontal?: boolean;
  topN?: number;
  unit?: string;
  yMax?: number;
  gap?: number;
}

function formatNum(v: number): string | number {
  if (v >= 1000) return (v / 1000).toFixed(v % 1000 === 0 ? 0 : 1) + "k";
  return Math.round(v * 100) / 100;
}

export function BarChart({
  data = [],
  labels = [],
  height = 180,
  color = "var(--zinc-500)",
  highlightColor = "var(--red-folk)",
  highlightLast = false,
  horizontal = false,
  topN,
  unit = "",
  yMax,
  gap = 0.25,
  className,
  ...props
}: BarChartProps) {
  if (horizontal) {
    return (
      <RankedBars
        data={data}
        labels={labels}
        color={color}
        highlightColor={highlightColor}
        topN={topN}
        unit={unit}
        max={yMax}
        className={className}
        {...props}
      />
    );
  }
  const W = 640;
  const padL = 44;
  const padR = 12;
  const padT = 12;
  const padB = 26;
  const H = height;
  const iw = W - padL - padR;
  const ih = H - padT - padB;
  const max = yMax ?? (Math.max(...data, 0) * 1.15 || 1);
  const n = data.length || 1;
  const band = iw / n;
  const bw = band * (1 - gap);
  const yAt = (v: number) => padT + ih - (v / max) * ih;
  const ticks = 4;

  return (
    <div className={className} {...props}>
      <svg
        viewBox={`0 0 ${W} ${H}`}
        width="100%"
        height={H}
        role="img"
        preserveAspectRatio="xMidYMid meet"
        style={{ display: "block", overflow: "visible" }}
      >
        {Array.from({ length: ticks + 1 }).map((_, i) => {
          const v = (max / ticks) * (ticks - i);
          const y = padT + (ih / ticks) * i;
          return (
            <g key={i}>
              <line x1={padL} y1={y} x2={W - padR} y2={y} stroke="var(--border-subtle)" strokeWidth="1" />
              <text
                x={padL - 8}
                y={y + 3}
                textAnchor="end"
                fontSize="10"
                fontFamily="var(--font-mono)"
                fill="var(--text-subtle)"
              >
                {formatNum(v)}
                {unit}
              </text>
            </g>
          );
        })}
        {data.map((v, i) => {
          const x = padL + band * i + (band - bw) / 2;
          const y = yAt(v);
          const h = padT + ih - y;
          const fill = highlightLast && i === data.length - 1 ? highlightColor : color;
          return <rect key={i} x={x} y={y} width={bw} height={Math.max(h, 0)} rx="2" fill={fill} />;
        })}
        {labels.map((l, i) => (
          <text
            key={i}
            x={padL + band * i + band / 2}
            y={H - 8}
            textAnchor="middle"
            fontSize="10"
            fontFamily="var(--font-mono)"
            fill="var(--text-subtle)"
          >
            {l}
          </text>
        ))}
      </svg>
    </div>
  );
}

interface RankedBarsProps extends React.HTMLAttributes<HTMLDivElement> {
  data: number[];
  labels: (string | number)[];
  color: string;
  highlightColor: string;
  topN?: number;
  unit: string;
  max?: number;
}

// horizontal ranked bars — the readable pattern for 10+ categories.
function RankedBars({
  data,
  labels,
  color,
  highlightColor,
  topN,
  unit,
  max,
  className,
  ...props
}: RankedBarsProps) {
  let rows = data.map((v, i) => ({ v, label: labels[i] != null ? labels[i] : String(i) }));
  rows.sort((a, b) => b.v - a.v);
  if (topN) rows = rows.slice(0, topN);
  const mx = max ?? (Math.max(...rows.map((r) => r.v), 0) || 1);
  return (
    <div
      className={className}
      style={{ display: "flex", flexDirection: "column", gap: 6 }}
      {...props}
    >
      {rows.map((r, i) => (
        <div key={i} style={{ display: "flex", alignItems: "center", gap: 10 }}>
          <span
            style={{
              width: 96,
              flex: "none",
              textAlign: "right",
              fontFamily: "var(--font-mono)",
              fontSize: "var(--text-xs)",
              color: "var(--text-muted)",
              overflow: "hidden",
              textOverflow: "ellipsis",
              whiteSpace: "nowrap",
            }}
          >
            {r.label}
          </span>
          <div
            style={{
              flex: 1,
              height: 16,
              background: "var(--surface-subtle)",
              borderRadius: 3,
              overflow: "hidden",
            }}
          >
            <div
              style={{
                width: (r.v / mx) * 100 + "%",
                height: "100%",
                background: i === 0 ? highlightColor : color,
                borderRadius: 3,
              }}
            />
          </div>
          <span
            style={{
              width: 56,
              flex: "none",
              fontFamily: "var(--font-mono)",
              fontSize: "var(--text-xs)",
              color: "var(--text-secondary)",
            }}
          >
            {formatNum(r.v)}
            {unit}
          </span>
        </div>
      ))}
    </div>
  );
}
