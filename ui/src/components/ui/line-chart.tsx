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
  /**
   * Rendered instead of the axes when there is nothing to plot. A chart with no
   * data drew a full grid and a flat line along zero, which reads as a plotted
   * result rather than as an absence — on a cost panel those are different
   * facts (#960). Callers pass their own translated copy; without one the
   * chart still refuses to draw an axis it has nothing to put against.
   */
  emptyState?: React.ReactNode;
  /**
   * Accessible name for the graphic. `role="img"` promises a name, and axe
   * fails the story when there is none (#1181); a chart the caller does not
   * name is treated as decorative instead, because the heading and the figures
   * beside it already carry the fact. Pass a translated string.
   */
  label?: string;
}

// the first three of the shared categorical sequence (#1245)
const DEFAULT_COLORS = ["var(--chart-1)", "var(--chart-2)", "var(--chart-3)"];

/// small dependency-free SVG line chart — ported from the design system's
/// components/charts/LineChart.jsx, driven by the same CSS custom properties
/// (--border-subtle, --text-secondary, --font-mono) already defined in
/// ui/src/index.css so it matches the rest of the dashboard without pulling
/// in a charting library
export function LineChart({
  series,
  labels,
  height = 180,
  formatValue,
  emptyState,
  label,
}: LineChartProps) {
  const width = 640;
  // the left gutter holds the y-axis labels, and the horizontal padding has to
  // be wide enough for half of the first and last tick label: with the plot
  // area flush to the edge they were sliced at both ends — `13:00` read as
  // `3:00` and `21:00` lost its last character (#960)
  const padding = { top: 12, right: 26, bottom: 24, left: 44 };
  const innerWidth = width - padding.left - padding.right;
  const innerHeight = height - padding.top - padding.bottom;

  const allValues = series.flatMap((s) => s.values).filter((v) => Number.isFinite(v));
  // nothing to plot: no series, no points, or nothing finite in them
  const hasData = allValues.length > 0 && labels.length > 0;

  const max = Math.max(1, ...allValues);
  const min = Math.min(0, ...allValues);
  const span = max - min || 1;

  const pointCount = labels.length;
  const xFor = (i: number) =>
    padding.left + (pointCount <= 1 ? innerWidth / 2 : (innerWidth * i) / (pointCount - 1));
  const yFor = (v: number) => padding.top + innerHeight - ((v - min) / span) * innerHeight;

  const pathFor = (values: number[]) =>
    values.map((v, i) => `${i === 0 ? "M" : "L"} ${xFor(i)} ${yFor(v)}`).join(" ");

  // thin every label out so the axis stays readable at any width
  const labelStride = Math.max(1, Math.ceil(pointCount / 6));

  // one tick per gridline, top down, so a point can be read against a scale
  // rather than against the single max label the chart used to carry
  const ticks = [0, 0.25, 0.5, 0.75, 1];

  if (!hasData) {
    return (
      <div
        className="flex w-full items-center justify-center"
        style={{ minHeight: height }}
        role="status"
      >
        {emptyState}
      </div>
    );
  }

  return (
    <svg
      viewBox={`0 0 ${width} ${height}`}
      className="h-full w-full"
      {...(label ? { role: "img", "aria-label": label } : { "aria-hidden": true })}
    >
      {/* horizontal gridlines, each with the value it sits at */}
      {ticks.map((t) => {
        const y = padding.top + innerHeight * t;
        return (
          <g key={t}>
            <line
              x1={padding.left}
              x2={width - padding.right}
              y1={y}
              y2={y}
              stroke="var(--border-subtle)"
              strokeWidth={1}
            />
            {formatValue ? (
              <text
                x={padding.left - 6}
                // +3 centres a 9px glyph on the line it labels
                y={y + 3}
                textAnchor="end"
                fontSize={9}
                fontFamily="var(--font-mono)"
                fill="var(--text-secondary)"
              >
                {formatValue(max - span * t)}
              </text>
            ) : null}
          </g>
        );
      })}

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
    </svg>
  );
}
