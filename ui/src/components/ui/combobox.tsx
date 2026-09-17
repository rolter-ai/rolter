import { Check, ChevronDown, X } from "lucide-react";
import * as React from "react";
import { useTranslation } from "react-i18next";

import { cn } from "@/lib/utils";

// type-to-filter combobox, the styled replacement for the native <select> the
// dashboard shipped until #968. A native select draws its open list in the
// operating system, so the design tokens stop at the closed control and there
// is no filtering beyond first-letter jumping — unusable once a list is a
// fleet of `provider/model` addresses.
//
// The pattern is the APG "editable combobox with list autocomplete": the input
// *is* the combobox, DOM focus never leaves it, and the active option is named
// with aria-activedescendant. That is what keeps it safe inside a Sheet or a
// Dialog — the modal Tab trap sees focus stay in the panel, because the popup
// never takes it (see lib/modal-a11y.ts).

export interface ComboboxOption {
  value: string;
  /** what the row reads as, and what the filter matches against */
  label: string;
  /** secondary line under the label — the model picker shows `owned_by` here */
  description?: string;
  /** optional group header this option sits under, in first-seen order */
  group?: string;
  disabled?: boolean;
}

export interface ComboboxProps {
  options: ComboboxOption[];
  /** selected value; `""` is "nothing selected" */
  value: string;
  onChange: (value: string) => void;
  /** shown when nothing is selected; defaults to the catalog placeholder */
  placeholder?: string;
  /** offer an × that resets the selection — for an optional field */
  clearable?: boolean;
  disabled?: boolean;
  /**
   * control height. `default` matches Input; `sm` is the compact toolbar
   * variant the dashboard wrote as `h-8 text-xs` on the native select
   */
  size?: "default" | "sm";
  id?: string;
  name?: string;
  /** native tooltip, for a compact control whose label is elsewhere */
  title?: string;
  /** layout classes for the wrapper — margins, width, grid placement */
  className?: string;
  /** extra classes for the popup, mainly to widen it past the control */
  listClassName?: string;
  "aria-label"?: string;
  "aria-describedby"?: string;
  "aria-invalid"?: boolean | "true" | "false";
}

/** substring match, case- and diacritic-insensitive, over label + description */
function matches(option: ComboboxOption, query: string): boolean {
  if (!query) return true;
  const needle = fold(query);
  return (
    fold(option.label).includes(needle) ||
    fold(option.value).includes(needle) ||
    fold(option.description ?? "").includes(needle)
  );
}

function fold(text: string): string {
  return text.normalize("NFD").replace(/\p{Diacritic}/gu, "").toLowerCase();
}

/**
 * The popup laid out as sections.
 *
 * A grouped option becomes `listbox > group > option`, which is the only shape
 * ARIA allows a header in — a bare heading between options is a
 * `aria-required-children` failure, and a listbox that owns nothing else has
 * no way to name a run of rows.
 */
interface Section {
  label?: string;
  items: { option: ComboboxOption; index: number }[];
}

/**
 * Rows rendered at once, above which the popup windows instead (#1579).
 *
 * Below it every filtered option is a DOM node, which is both simpler and
 * indistinguishable in practice — the type-to-filter narrows a fleet to a
 * handful in two keystrokes. Above it a deployment with thousands of
 * `provider/model` addresses pays for the whole list on every keystroke.
 */
const VIRTUALISE_ABOVE = 120;

/** rows kept beyond each edge of the viewport, so a fast scroll has something */
const OVERSCAN = 8;

/**
 * Row heights, in px, written here rather than measured.
 *
 * A variable-height virtual list normally has to render a row to find out how
 * tall it is, and then correct the scroll it already reported. Here it does
 * not have to: a row is tall exactly when the option carries a `description`,
 * and that is known from the data. The numbers are applied to the rows as an
 * inline `height`, so the arithmetic and the layout cannot drift apart — the
 * rows are what these say they are, rather than these being a guess at what
 * the classes produce.
 */
const ROW = {
  default: { plain: 32, described: 48 },
  sm: { plain: 28, described: 44 },
} as const;
const HEADER_HEIGHT = 26;

/** one thing the popup stacks vertically: a group's header, or an option */
interface Row {
  kind: "header" | "option";
  /** section this row belongs to, so a window can rebuild the group wrappers */
  section: number;
  option?: ComboboxOption;
  /** index into the filtered list — what `active` and the option id speak in */
  index: number;
  top: number;
  height: number;
}

/** every row with its offset, which is all the window needs to place itself */
function measure(sections: Section[], size: "default" | "sm"): { rows: Row[]; total: number } {
  const rows: Row[] = [];
  let top = 0;
  sections.forEach((section, s) => {
    if (section.label) {
      rows.push({ kind: "header", section: s, index: -1, top, height: HEADER_HEIGHT });
      top += HEADER_HEIGHT;
    }
    for (const { option, index } of section.items) {
      const height = ROW[size][option.description ? "described" : "plain"];
      rows.push({ kind: "option", section: s, option, index, top, height });
      top += height;
    }
  });
  return { rows, total: top };
}

/**
 * The window's rows, split back into the sections they belong to.
 *
 * A group's header only renders when its own row is inside the window; the
 * wrapper still carries `aria-label`, so a run of options scrolled past its
 * heading is announced under the right name either way.
 */
function slices(rows: Row[], first: number, last: number) {
  const out: { section: number; header: boolean; items: Row[] }[] = [];
  for (let i = first; i <= last; i += 1) {
    const r = rows[i];
    const open = out[out.length - 1];
    if (r.kind === "header") {
      out.push({ section: r.section, header: true, items: [] });
      continue;
    }
    if (open && open.section === r.section) open.items.push(r);
    else out.push({ section: r.section, header: false, items: [r] });
  }
  return out.filter((slice) => slice.items.length > 0);
}

/** the first row at or after `offset`, by binary search over the offsets */
function rowAt(rows: Row[], offset: number): number {
  let lo = 0;
  let hi = rows.length - 1;
  while (lo < hi) {
    const mid = (lo + hi) >> 1;
    if (rows[mid].top + rows[mid].height <= offset) lo = mid + 1;
    else hi = mid;
  }
  return lo;
}

function layout(options: ComboboxOption[]): Section[] {
  const sections: Section[] = [];
  options.forEach((option, index) => {
    const last = sections[sections.length - 1];
    if (last && last.label === option.group) last.items.push({ option, index });
    else sections.push({ label: option.group, items: [{ option, index }] });
  });
  return sections;
}

export const Combobox = React.forwardRef<HTMLInputElement, ComboboxProps>(
  function Combobox(
    {
      options,
      value,
      onChange,
      placeholder,
      clearable = false,
      disabled = false,
      size = "default",
      id,
      name,
      title,
      className,
      listClassName,
      "aria-label": ariaLabel,
      "aria-describedby": describedBy,
      "aria-invalid": invalid,
    },
    ref,
  ) {
    const { t } = useTranslation();
    const generatedId = React.useId();
    const inputId = id ?? generatedId;
    const listId = `${generatedId}-list`;
    const statusId = `${generatedId}-status`;

    const [open, setOpen] = React.useState(false);
    // null means "showing the selection"; a string means the user is filtering
    const [query, setQuery] = React.useState<string | null>(null);
    const [active, setActive] = React.useState(0);
    const [above, setAbove] = React.useState(false);

    const [scrollTop, setScrollTop] = React.useState(0);
    const [viewport, setViewport] = React.useState(240);

    const wrapper = React.useRef<HTMLDivElement>(null);
    const input = React.useRef<HTMLInputElement>(null);
    const list = React.useRef<HTMLDivElement>(null);
    const listbox = React.useRef<HTMLDivElement>(null);
    React.useImperativeHandle(ref, () => input.current as HTMLInputElement);

    const selected = options.find((o) => o.value === value);
    const filtered = React.useMemo(
      () => (query ? options.filter((o) => matches(o, query)) : options),
      [options, query],
    );
    const sections = React.useMemo(() => layout(filtered), [filtered]);
    const enabled = React.useMemo(
      () => filtered.map((o, i) => (o.disabled ? -1 : i)).filter((i) => i >= 0),
      [filtered],
    );

    // above the threshold the popup renders a window of rows rather than all of
    // them (#1579). the offsets come from the data, since a row is tall exactly
    // when its option has a description
    const virtual = filtered.length > VIRTUALISE_ABOVE;
    const { rows, total } = React.useMemo(
      () => (virtual ? measure(sections, size) : { rows: [], total: 0 }),
      [virtual, sections, size],
    );
    const window_ = React.useMemo(() => {
      if (!virtual) return null;
      let first = Math.max(0, rowAt(rows, scrollTop) - OVERSCAN);
      let last = Math.min(rows.length - 1, rowAt(rows, scrollTop + viewport) + OVERSCAN);
      // the active option must be a rendered node whatever the scroll says, or
      // aria-activedescendant names an id that is not in the document and the
      // screen reader announces nothing — the one regression #968 must not have
      const at = rows.findIndex((r) => r.kind === "option" && r.index === active);
      if (at >= 0) {
        first = Math.min(first, at);
        last = Math.max(last, at);
      }
      return { first, last };
    }, [virtual, rows, scrollTop, viewport, active]);

    const optionId = (index: number) => `${generatedId}-opt-${index}`;
    // what the control reads while closed is the selection, not the last query
    const text = query ?? selected?.label ?? "";

    const close = React.useCallback(() => {
      setOpen(false);
      setQuery(null);
    }, []);

    const commit = React.useCallback(
      (option: ComboboxOption | undefined) => {
        if (!option || option.disabled) return;
        onChange(option.value);
        close();
      },
      [close, onChange],
    );

    // opening decides which way the popup goes: below unless the control sits
    // low enough that the list would run off the bottom of the viewport
    const show = React.useCallback(() => {
      if (disabled) return;
      const box = wrapper.current?.getBoundingClientRect();
      if (box) {
        const below = window.innerHeight - box.bottom;
        setAbove(below < 240 && box.top > below);
      }
      // the field empties so the next keystroke starts a filter rather than
      // editing the selected label at wherever the caret happened to land; the
      // selection stays visible as the placeholder and as the ticked row
      setQuery((current) => current ?? "");
      setOpen(true);
    }, [disabled]);

    // keep the active option in view while arrowing through a long list.
    // a layout effect rather than an effect: the window is computed from
    // `scrollTop`, so the scroll and the state that follows it have to settle
    // before the browser paints, or a long list flashes the old window
    React.useLayoutEffect(() => {
      if (!open) return;
      const node = list.current?.querySelector<HTMLElement>('[data-active="true"]');
      node?.scrollIntoView({ block: "nearest" });
      const box = listbox.current;
      if (box) {
        setScrollTop(box.scrollTop);
        if (box.clientHeight > 0) setViewport(box.clientHeight);
      }
    }, [open, active, filtered]);

    // the active option is reset whenever the candidate set changes, so the
    // first match is always the one Enter takes
    React.useEffect(() => {
      setActive(() => {
        if (!query) {
          const current = filtered.findIndex((o) => o.value === value);
          if (current >= 0) return current;
        }
        return enabled[0] ?? 0;
      });
    }, [query, filtered, enabled, value]);

    const move = (delta: number) => {
      if (enabled.length === 0) return;
      const at = enabled.indexOf(active);
      const next = at < 0 ? (delta > 0 ? 0 : enabled.length - 1) : at + delta;
      setActive(enabled[(next + enabled.length) % enabled.length]);
    };

    const onKeyDown = (event: React.KeyboardEvent<HTMLInputElement>) => {
      switch (event.key) {
        case "ArrowDown":
          event.preventDefault();
          if (!open) show();
          else move(1);
          return;
        case "ArrowUp":
          event.preventDefault();
          if (!open) show();
          else move(-1);
          return;
        case "Home":
          if (!open) return;
          event.preventDefault();
          setActive(enabled[0] ?? 0);
          return;
        case "End":
          if (!open) return;
          event.preventDefault();
          setActive(enabled[enabled.length - 1] ?? 0);
          return;
        case "Enter":
          if (!open) return;
          // a combobox in a form must not submit it on the keystroke that only
          // picks an option
          event.preventDefault();
          commit(filtered[active]);
          return;
        case "Escape":
          if (!open) return;
          // the popup closes, the Sheet or Dialog around it does not (#968)
          event.preventDefault();
          event.stopPropagation();
          close();
          return;
        case "Tab":
          if (open) close();
          return;
        default:
      }
    };

    const clear = () => {
      onChange("");
      setQuery(null);
      input.current?.focus();
    };

    const count = filtered.length;

    /** one option row. its height is pinned while windowing, since the offsets
        the window is computed from are these numbers and nothing else */
    const row = ({ option, index }: { option: ComboboxOption; index: number }) => (
      <div
        key={option.value}
        id={optionId(index)}
        role="option"
        aria-selected={option.value === value}
        aria-disabled={option.disabled || undefined}
        data-active={index === active}
        style={virtual ? { height: ROW[size][option.description ? "described" : "plain"] } : undefined}
        // mousedown rather than click: the default would blur the input and
        // close the popup before the click ever landed
        onMouseDown={(event) => {
          event.preventDefault();
          commit(option);
        }}
        onMouseMove={() => {
          if (!option.disabled) setActive(index);
        }}
        className={cn(
          "flex cursor-pointer items-start gap-2 rounded-[var(--radius-sm)] px-2 py-1.5",
          size === "sm" ? "text-xs" : "text-sm",
          index === active && "bg-[color:var(--surface-hover)]",
          option.disabled && "cursor-not-allowed opacity-50",
        )}
      >
        <Check
          aria-hidden
          className={cn(
            "mt-0.5 h-3.5 w-3.5 flex-none",
            option.value === value ? "opacity-100" : "opacity-0",
          )}
        />
        <span className="min-w-0 flex-1">
          <span className="block truncate">{option.label}</span>
          {option.description && (
            <span className="block truncate text-xs text-muted-foreground">
              {option.description}
            </span>
          )}
        </span>
      </div>
    );

    /** a run of options under their heading, or the bare run when ungrouped */
    const group = (label: string | undefined, children: React.ReactNode[], header = true) => {
      if (!label) return children;
      return (
        <div key={`${label}-${(children[0] as { key?: string })?.key ?? ""}`} role="group" aria-label={label}>
          {header && (
            <p
              aria-hidden
              style={virtual ? { height: HEADER_HEIGHT } : undefined}
              className="px-2 pb-1 pt-2 text-2xs font-medium uppercase tracking-wide text-muted-foreground"
            >
              {label}
            </p>
          )}
          {children}
        </div>
      );
    };

    return (
      <div
        ref={wrapper}
        className={cn("relative", className)}
        onBlur={(event) => {
          // focus left the control entirely — not a hop between its own parts
          if (!event.currentTarget.contains(event.relatedTarget as Node | null)) close();
        }}
      >
        {/* a form-serialisable mirror, so a Combobox drops into a <form> where
            a <select> used to be */}
        {name && <input type="hidden" name={name} value={value} />}
        <input
          ref={input}
          id={inputId}
          role="combobox"
          type="text"
          autoComplete="off"
          spellCheck={false}
          disabled={disabled}
          title={title}
          value={text}
          placeholder={
            (open && selected?.label) || placeholder || t("common.combobox.placeholder")
          }
          aria-label={ariaLabel}
          aria-describedby={describedBy}
          aria-invalid={invalid}
          aria-expanded={open}
          aria-controls={listId}
          aria-autocomplete="list"
          aria-activedescendant={
            open && filtered[active] ? optionId(active) : undefined
          }
          onChange={(event) => {
            setQuery(event.target.value);
            if (!open) show();
          }}
          onKeyDown={onKeyDown}
          onMouseDown={() => {
            if (open) close();
            else show();
          }}
          className={cn(
            "flex w-full rounded-md border border-input bg-[color:var(--surface-subtle)] px-3 py-1 transition-colors",
            "placeholder:text-muted-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring",
            "disabled:cursor-not-allowed disabled:opacity-50",
            size === "sm" ? "h-8 text-xs" : "h-9 text-sm",
            clearable && value ? "pr-14" : "pr-8",
          )}
        />
        {clearable && value && !disabled && (
          <button
            type="button"
            tabIndex={-1}
            aria-label={t("common.combobox.clear")}
            onClick={clear}
            className="absolute right-7 top-1/2 -translate-y-1/2 rounded p-0.5 text-muted-foreground transition-colors hover:text-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
          >
            <X className="h-3.5 w-3.5" />
          </button>
        )}
        <ChevronDown
          aria-hidden
          className={cn(
            "pointer-events-none absolute right-2.5 top-1/2 h-3.5 w-3.5 -translate-y-1/2 text-muted-foreground transition-transform",
            open && "rotate-180",
          )}
        />
        {/* the count is what a screen reader hears after each keystroke; it is
            polite so it never interrupts the character echo.

            `aria-live` without `role="status"` on purpose: the role would put a
            second status node on every screen that has a dropdown, and the
            screens whose own notice is a `role="status"` query for it by role —
            one combobox on the page and `getByRole("status")` stops being
            unambiguous. Live regions are announced from `aria-live` alone */}
        <p id={statusId} aria-live="polite" aria-atomic className="sr-only">
          {open ? t("common.combobox.results", { count }) : ""}
        </p>
        <div
          ref={list}
          hidden={!open}
          className={cn(
            "absolute z-50 w-full rounded-md border border-[color:var(--border-default)]",
            "bg-[color:var(--surface-elevated)] p-1 shadow-[var(--shadow-md)]",
            above ? "bottom-full mb-1" : "top-full mt-1",
            listClassName,
          )}
        >
          {/* the empty-result message is a sibling of the listbox, not a child
              of it: a listbox may only own options and groups, and a bare
              paragraph inside one is an axe `aria-required-children` failure */}
          {open && count === 0 && (
            <p className="px-2 py-3 text-center text-xs text-muted-foreground">
              {t("common.combobox.noMatches")}
            </p>
          )}
          <div
            ref={listbox}
            id={listId}
            role="listbox"
            onScroll={(event) => {
              if (!virtual) return;
              setScrollTop(event.currentTarget.scrollTop);
              if (event.currentTarget.clientHeight > 0) setViewport(event.currentTarget.clientHeight);
            }}
            // a generic name on purpose: naming the listbox after the field
            // would put a second node with that accessible name on the page,
            // and `getByLabelText("Provider")` would stop being unambiguous —
            // the combobox it belongs to is announced immediately before it
            aria-label={t("common.combobox.options")}
            // the scroll lives on the listbox rather than on the popup around
            // it: a scrollable plain <div> is an axe `scrollable-region-focusable`
            // failure, since axe cannot see that the arrow keys on the combobox
            // are what scrolls it. as a listbox it is a widget axe understands
            className="max-h-60 overflow-y-auto"
          >
            {/* the rows exist only while the popup is open. a closed combobox
                that kept them left every option's text in the document, so a
                screen with a dropdown of audit actions had two nodes reading
                `provider.create` and `getByText` stopped being unambiguous —
                and a long list paid its DOM cost on every render */}
            {open && !virtual && sections.map((section) => group(section.label, section.items.map(row)))}
            {open && virtual && window_ && (
              <>
                {/* the rows above and below the window are one box each. they
                    carry role="none" so the listbox still owns nothing but
                    options and groups, which is what aria-required-children
                    asks of it */}
                <div role="none" style={{ height: rows[window_.first].top }} />
                {slices(rows, window_.first, window_.last).map((slice) =>
                  group(
                    sections[slice.section].label,
                    slice.items.map(({ option, index }) => row({ option: option!, index })),
                    slice.header,
                  ),
                )}
                <div
                  role="none"
                  style={{ height: total - (rows[window_.last].top + rows[window_.last].height) }}
                />
              </>
            )}
          </div>
        </div>
      </div>
    );
  },
);
