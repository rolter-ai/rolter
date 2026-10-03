import * as React from "react";
import { useTranslation } from "react-i18next";

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
   * beside it already carry the fact. Pass a translated string. A named chart
   * also carries a visually hidden table of the values it plots, captioned with
   * this label, since the graphic alone gives assistive technology a name and
   * no numbers (#2005).
   */
  label?: string;
}

// the first three of the shared categorical sequence (#1245)
const DEFAULT_COLORS = ["var(--chart-1)", "var(--chart-2)", "var(--chart-3)"];

// the width drawn before the frame has been measured, and wherever it cannot be
// (no layout, no ResizeObserver)
const FALLBACK_WIDTH = 640;
// axis text, in the pixels it is read at: the `--text-2xs` size. the viewBox is
// as wide as the frame, so a unit is a pixel at every width
const FONT = 11;
// the advance of one Geist Mono glyph, which is what a tick label is set in
const GLYPH = 0.6 * FONT;
// the most x labels the axis carries, however much room there is
const MAX_X_LABELS = 6;

/// small dependency-free SVG line chart, driven by the same CSS custom
/// properties (--border-subtle, --text-secondary, --font-mono) already defined in
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
  const { t } = useTranslation();
  const allValues = series.flatMap((s) => s.values).filter((v) => Number.isFinite(v));
  // nothing to plot: no series, no points, or nothing finite in them
  const hasData = allValues.length > 0 && labels.length > 0;

  // the chart is drawn at the width it is given. a fixed 640-wide viewBox
  // scaled down to a phone shrank the axis text to a third of its size, and the
  // plot to a sliver, along with it (#1994)
  const frame = React.useRef<HTMLDivElement>(null);
  const [measured, setMeasured] = React.useState(0);
  React.useLayoutEffect(() => {
    const el = frame.current;
    if (!el) return;
    const read = () => setMeasured(el.clientWidth);
    read();
    if (typeof ResizeObserver === "undefined") return;
    const observer = new ResizeObserver(read);
    observer.observe(el);
    return () => observer.disconnect();
  }, [hasData]);
  const width = measured > 0 ? measured : FALLBACK_WIDTH;

  const max = Math.max(1, ...allValues);
  const min = Math.min(0, ...allValues);
  const span = max - min || 1;

  // one tick per gridline, top down, so a point can be read against a scale
  // rather than against the single max label the chart used to carry
  const ticks = [0, 0.25, 0.5, 0.75, 1];
  const tickLabels = formatValue ? ticks.map((t) => formatValue(max - span * t)) : [];

  // the left gutter holds the y-axis labels, so it is as wide as the longest of
  // them (`$1,234.50` outgrows a fixed 44), and the horizontal padding has to be
  // wide enough for half of the first and last tick label: with the plot area
  // flush to the edge they were sliced at both ends — `13:00` read as `3:00` and
  // `21:00` lost its last character (#960)
  const longestTick = Math.max(0, ...tickLabels.map((l) => l.length));
  const padding = {
    top: 12,
    right: 26,
    bottom: 24,
    left: Math.max(44, Math.ceil(longestTick * GLYPH) + 10),
  };
  const innerWidth = Math.max(1, width - padding.left - padding.right);
  const innerHeight = height - padding.top - padding.bottom;

  const pointCount = labels.length;
  const xFor = (i: number) =>
    padding.left + (pointCount <= 1 ? innerWidth / 2 : (innerWidth * i) / (pointCount - 1));
  const yFor = (v: number) => padding.top + innerHeight - ((v - min) / span) * innerHeight;

  const pathFor = (values: number[]) =>
    values.map((v, i) => `${i === 0 ? "M" : "L"} ${xFor(i)} ${yFor(v)}`).join(" ");

  // thin the labels out to what the axis has room for, so they never run into
  // one another: six across a desktop card, three across a phone
  const widest = Math.max(1, ...labels.map((l) => l.length)) * GLYPH;
  const fits = Math.max(1, Math.floor(innerWidth / (widest + 12)));
  const labelStride = Math.max(1, Math.ceil(pointCount / Math.min(MAX_X_LABELS, fits)));

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
    <div ref={frame} className="w-full">
      <svg
        viewBox={`0 0 ${width} ${height}`}
        width="100%"
        height={height}
        className="block"
        {...(label ? { role: "img", "aria-label": label } : { "aria-hidden": true })}
      >
        {/* horizontal gridlines, each with the value it sits at */}
        {ticks.map((t, ti) => {
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
                  // centres the glyph on the line it labels
                  y={y + 0.35 * FONT}
                  textAnchor="end"
                  fontSize={FONT}
                  fontFamily="var(--font-mono)"
                  fill="var(--text-secondary)"
                >
                  {tickLabels[ti]}
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
              fontSize={FONT}
              fontFamily="var(--font-mono)"
              fill="var(--text-secondary)"
            >
              {label}
            </text>
          ) : null,
        )}
      </svg>
      {label ? (
        <table className="sr-only">
          <caption>{label}</caption>
          <thead>
            <tr>
              <th scope="col">{t("common.chartPoint")}</th>
              {series.map((s) => (
                <th key={s.name} scope="col">
                  {s.name}
                </th>
              ))}
            </tr>
          </thead>
          <tbody>
            {labels.map((point, i) => (
              <tr key={`${point}-${i}`}>
                <th scope="row">{point}</th>
                {series.map((s) => {
                  const value = s.values[i];
                  const known = value !== undefined && Number.isFinite(value);
                  return (
                    <td key={s.name}>{known ? (formatValue ? formatValue(value) : value) : ""}</td>
                  );
                })}
              </tr>
            ))}
          </tbody>
        </table>
      ) : null}
    </div>
  );
}
