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

    const wrapper = React.useRef<HTMLDivElement>(null);
    const input = React.useRef<HTMLInputElement>(null);
    const list = React.useRef<HTMLDivElement>(null);
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

    // keep the active option in view while arrowing through a long list
    React.useEffect(() => {
      if (!open) return;
      const node = list.current?.querySelector<HTMLElement>('[data-active="true"]');
      node?.scrollIntoView({ block: "nearest" });
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
          {count === 0 && (
            <p className="px-2 py-3 text-center text-xs text-muted-foreground">
              {t("common.combobox.noMatches")}
            </p>
          )}
          <div
            id={listId}
            role="listbox"
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
            {sections.map((section) => {
              const options = section.items.map(({ option, index }) => (
                <div
                  key={option.value}
                  id={optionId(index)}
                  role="option"
                  aria-selected={option.value === value}
                  aria-disabled={option.disabled || undefined}
                  data-active={index === active}
                  // mousedown rather than click: the default would blur the
                  // input and close the popup before the click ever landed
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
              ));
              if (!section.label) return options;
              return (
                <div key={section.label} role="group" aria-label={section.label}>
                  <p
                    aria-hidden
                    className="px-2 pb-1 pt-2 text-2xs font-medium uppercase tracking-wide text-muted-foreground"
                  >
                    {section.label}
                  </p>
                  {options}
                </div>
              );
            })}
          </div>
        </div>
      </div>
    );
  },
);
