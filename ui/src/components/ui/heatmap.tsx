// intensity grid (request volume by hour x day), Grafana-style. cell color
// scales from surface to folk red by value. ported from the Rolter Design
// System components/charts/Heatmap.jsx
export interface HeatmapProps extends React.HTMLAttributes<HTMLDivElement> {
  matrix: number[][];
  rowLabels?: (string | number)[];
  colLabels?: (string | number)[];
  max?: number;
  cell?: number;
  gap?: number;
}

export function Heatmap({
  matrix = [],
  rowLabels = [],
  colLabels = [],
  max,
  cell = 22,
  gap = 3,
  className,
  ...props
}: HeatmapProps) {
  const rows = matrix.length;
  const cols = rows ? matrix[0].length : 0;
  const hi = max ?? Math.max(1, ...matrix.flat());
  const labelW = 34;
  const labelH = 16;
  const W = labelW + cols * (cell + gap);
  const H = labelH + rows * (cell + gap);
  const shade = (v: number) => {
    if (v <= 0) return "var(--surface-subtle)";
    const t = Math.min(v / hi, 1);
    return `color-mix(in oklab, var(--red-folk) ${Math.round(12 + t * 88)}%, var(--surface-subtle))`;
  };
  return (
    <div className={className} style={{ overflowX: "auto" }} {...props}>
      <svg viewBox={`0 0 ${W} ${H}`} width={W} height={H} style={{ display: "block", maxWidth: "100%" }}>
        {colLabels.map((l, c) => (
          <text
            key={c}
            x={labelW + c * (cell + gap) + cell / 2}
            y={labelH - 5}
            textAnchor="middle"
            fontSize="9"
            fontFamily="var(--font-mono)"
            fill="var(--text-subtle)"
          >
            {l}
          </text>
        ))}
        {matrix.map((row, r) => (
          <g key={r}>
            <text
              x={labelW - 8}
              y={labelH + r * (cell + gap) + cell / 2 + 3}
              textAnchor="end"
              fontSize="9"
              fontFamily="var(--font-mono)"
              fill="var(--text-subtle)"
            >
              {rowLabels[r]}
            </text>
            {row.map((v, c) => (
              <rect
                key={c}
                x={labelW + c * (cell + gap)}
                y={labelH + r * (cell + gap)}
                width={cell}
                height={cell}
                rx="2"
                fill={shade(v)}
              >
                <title>{`${rowLabels[r] ?? r} · ${colLabels[c] ?? c}: ${v}`}</title>
              </rect>
            ))}
          </g>
        ))}
      </svg>
    </div>
  );
}
