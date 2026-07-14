export interface LineChartSeries {
  name: string;
  values: number[];
  color?: string;
}

interface LineChartProps {
  series: LineChartSeries[];
  labels: string[];
  height?: number;
  formatValue?: (value: number) => string;
}

const DEFAULT_COLORS = ["var(--red-folk)", "var(--zinc-300)", "var(--status-info)"];

/// small dependency-free SVG line chart — ported from the design system's
/// components/charts/LineChart.jsx, driven by the same CSS custom properties
/// (--border-subtle, --text-secondary, --font-mono) already defined in
/// ui/src/index.css so it matches the rest of the dashboard without pulling
/// in a charting library
export function LineChart({ series, labels, height = 180, formatValue }: LineChartProps) {
  const width = 640;
  const padding = { top: 12, right: 12, bottom: 24, left: 12 };
  const innerWidth = width - padding.left - padding.right;
  const innerHeight = height - padding.top - padding.bottom;

  const allValues = series.flatMap((s) => s.values);
  const max = Math.max(1, ...allValues);
  const min = Math.min(0, ...allValues);
  const span = max - min || 1;

  const pointCount = labels.length;
  const xFor = (i: number) =>
    padding.left + (pointCount <= 1 ? 0 : (innerWidth * i) / (pointCount - 1));
  const yFor = (v: number) => padding.top + innerHeight - ((v - min) / span) * innerHeight;

  const pathFor = (values: number[]) =>
    values.map((v, i) => `${i === 0 ? "M" : "L"} ${xFor(i)} ${yFor(v)}`).join(" ");

  // thin every label out so the axis stays readable at any width
  const labelStride = Math.max(1, Math.ceil(pointCount / 6));

  return (
    <svg
      viewBox={`0 0 ${width} ${height}`}
      className="h-full w-full"
      role="img"
      aria-label="time series chart"
    >
      {/* horizontal gridlines */}
      {[0, 0.25, 0.5, 0.75, 1].map((t) => (
        <line
          key={t}
          x1={padding.left}
          x2={width - padding.right}
          y1={padding.top + innerHeight * t}
          y2={padding.top + innerHeight * t}
          stroke="var(--border-subtle)"
          strokeWidth={1}
        />
      ))}

      {series.map((s, si) => (
        <path
          key={s.name}
          d={pathFor(s.values)}
          fill="none"
          stroke={s.color ?? DEFAULT_COLORS[si % DEFAULT_COLORS.length]}
          strokeWidth={1.75}
          strokeLinejoin="round"
          strokeLinecap="round"
        />
      ))}

      {labels.map((label, i) =>
        i % labelStride === 0 ? (
          <text
            key={label}
            x={xFor(i)}
            y={height - 6}
            textAnchor="middle"
            fontSize={9}
            fontFamily="var(--font-mono)"
            fill="var(--text-secondary)"
          >
            {label}
          </text>
        ) : null,
      )}

      {formatValue && series[0] && series[0].values.length > 0 ? (
        <text
          x={padding.left}
          y={padding.top + 8}
          fontSize={9}
          fontFamily="var(--font-mono)"
          fill="var(--text-secondary)"
        >
          {formatValue(max)}
        </text>
      ) : null}
    </svg>
  );
}
