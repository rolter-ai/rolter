import type { Meta, StoryObj } from "@storybook/react-vite";
import { expect, userEvent, waitFor, within } from "storybook/test";

import Pricing from "./Pricing";
import {
  Harness,
  cancelConfirmation,
  clickWhenEnabled,
  confirmDestructive,
  expectAllowed,
  expectEmptyState,
  expectRefused,
  NEEDS_SUPERADMIN,
  expectAnalyticsUnavailable,
  expectListTable,
  expectLoadError,
  expectNoFalseEmpty,
  expectNoUxEvent,
  expectSheetClosed,
  expectSkeleton,
  expectUxEvent,
  json,
  openOptions,
  pending,
  pickOption,
  recordUxEvents,
  recording,
  routes,
  scoped,
  sheet,
  Toasted,
  expectToast,
  uxEvents,
  type Recorder,
  AddressProbe,
  addressOf,
} from "./story-harness";
import type { CurrencySettings, ModelPriceRow, PriceCatalogue } from "@/lib/api";
import ru from "@/lib/i18n/locales/ru.json";
import en from "@/lib/i18n/locales/en.json";
import { expectFieldHints } from "@/lib/story-hints";
import { UxScreenProvider } from "@/lib/ux-react";

const price = (
  model: string,
  currency: string,
  id = model,
  cacheWrite: string | null = null,
  cacheWrite1h: string | null = null,
): ModelPriceRow => ({
  id,
  model,
  input_per_mtok: "2.50",
  output_per_mtok: "10.00",
  cached_input_per_mtok: "1.25",
  cache_write_per_mtok: cacheWrite,
  cache_write_1h_per_mtok: cacheWrite1h,
  image_per_unit: null,
  audio_input_per_minute: null,
  speech_per_mchar: null,
  currency,
  created_at: "2026-07-01T00:00:00Z",
});

const MEDIA_PRICES: ModelPriceRow[] = [
  {
    ...price("dall-e-3", "USD"),
    input_per_mtok: "0.000000",
    output_per_mtok: "0.000000",
    cached_input_per_mtok: null,
    image_per_unit: "0.040000",
    audio_input_per_minute: "0.006000",
    speech_per_mchar: "15.000000",
  },
];

const PRICES: ModelPriceRow[] = [
  price("gpt-4o", "USD"),
  price("mistral-large", "EUR"),
  // priced in a code this deployment's rate table carries — #965's whole point
  price("yandex-gpt", "RUB"),
  // the control plane returns a rate as decimal text, six places (#2876)
  price("claude-sonnet", "USD", "claude-sonnet", "3.750000"),
];

/** prices that set the 1 hour write rate: beside a 5 minute one, and alone (#2902) */
const SPLIT_PRICES: ModelPriceRow[] = [
  price("claude-opus", "USD", "claude-opus", "3.750000", "6.000000"),
  price("claude-haiku", "USD", "claude-haiku", null, "2.000000"),
  price("claude-sonnet", "USD", "claude-sonnet", "3.750000"),
];

/** a deployment that settles in USD and has configured a RUB rate */
const CONFIGURED: CurrencySettings = {
  base: "USD",
  codes: ["USD", "EUR", "RUB"],
  rates: { USD: 1, EUR: 1.09, RUB: 0.011 },
};

/** the same prices against a table that has since dropped EUR and RUB */
const NARROWED: CurrencySettings = {
  base: "USD",
  codes: ["USD"],
  rates: { USD: 1 },
};

/** a by-model rollup row; the traffic a price is picked from, and what is unpriced */
const traffic = (model: string, unpriced = 0, org_id = "") => ({
  org_id,
  model,
  requests: 40,
  unpriced_requests: unpriced,
});

const SEEN_IN_TRAFFIC = [traffic("gpt-4o"), traffic("llama-3-70b")];

const withCurrency = (settings: CurrencySettings, prices = PRICES, seen = SEEN_IN_TRAFFIC) =>
  routes([
    ["/api/v1/currency", () => settings],
    [
      "/api/v1/models",
      () => [
        { model: "gpt-4o", strategy: "round_robin", targets: 2, source: "db" },
        { model: "claude-sonnet", strategy: "round_robin", targets: 1, source: "config" },
      ],
    ],
    // seen in traffic: one that is also a route, one that is not
    ["/api/v1/analytics/by-model", () => ({ data: seen })],
    ["/model-prices", () => prices],
  ]);

const meta = {
  title: "Screens/Pricing",
  component: Pricing,
  parameters: { layout: "fullscreen" },
} satisfies Meta<typeof Pricing>;
export default meta;
type Story = StoryObj<typeof meta>;

export const Loaded: Story = {
  render: () => (
    <Harness fetchStub={withCurrency(CONFIGURED)}>
      <Pricing />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    // every price converts, so nothing is flagged
    await expect(await canvas.findByText("gpt-4o")).toBeInTheDocument();
    await expect(canvas.queryByText(/no conversion rate/i)).not.toBeInTheDocument();
    await expectListTable(canvasElement, "Model Prices");
    // nothing carries unpriced traffic, so there is no list of it
    await expect(canvas.queryByRole("table", { name: "Models with unpriced traffic" })).toBeNull();
  },
};

const rowOf = async (canvasElement: HTMLElement, model: string) =>
  within((await within(canvasElement).findByText(model)).closest<HTMLElement>('[role="row"]')!);

/**
 * #2097: a price is a figure in a column, in mono, written the way the
 * dashboard's locale writes a number, with the currency in a column of its own.
 */
export const ShowsPricesAsFigures: Story = {
  render: () => (
    <Harness fetchStub={withCurrency(CONFIGURED)}>
      <Pricing />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const row = await rowOf(canvasElement, "gpt-4o");
    const input = row.getByText("2.50");
    await expect(input).toHaveClass("font-mono");
    await expect(input).toHaveClass("text-right");
    await expect(row.getByText("10.00")).toBeInTheDocument();
    await expect(row.getByText("1.25")).toBeInTheDocument();
    await expect(row.getByText("USD")).toBeInTheDocument();
    // no rate set for the cache writes, the image, audio and speech units, or the audio tokens
    await expect(row.getAllByText("—")).toHaveLength(8);
  },
};

/** #2097: the same figures in `ru` follow the dashboard's decimal comma */
export const ReadsInRussian: Story = {
  globals: { locale: "ru" },
  render: () => (
    <Harness fetchStub={withCurrency(CONFIGURED)}>
      <Pricing />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const row = await rowOf(canvasElement, "gpt-4o");
    await expect(row.getByText("2,50")).toBeVisible();
    await expect(row.getByText("10,00")).toBeVisible();
    await expect(row.queryByText("2.50")).toBeNull();
    await expect(
      within(canvasElement).getByRole("columnheader", { name: ru.pages.pricing.columns.input }),
    ).toBeVisible();
  },
};

const modelOrder = (canvasElement: HTMLElement) =>
  [...canvasElement.querySelectorAll('[role="table"] [role="row"] [role="cell"]:first-child')].map(
    (cell) => cell.textContent?.split(/\s/)[0],
  );

/** #2097: any price column sorts the catalog, and a second press reverses it */
export const SortsByAPrice: Story = {
  render: () => (
    <Harness
      fetchStub={withCurrency(CONFIGURED, [
        { ...price("cheap", "USD"), output_per_mtok: "0.60" },
        { ...price("dear", "USD"), output_per_mtok: "60.00" },
        { ...price("middling", "USD"), output_per_mtok: "10.00" },
      ])}
    >
      <Pricing />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await canvas.findByText("cheap");
    const header = canvas.getByRole("columnheader", { name: "Output" });
    await userEvent.click(within(header).getByRole("button"));
    await waitFor(() => expect(header).toHaveAttribute("aria-sort", "ascending"));
    await expect(modelOrder(canvasElement)).toEqual(["cheap", "middling", "dear"]);
    await userEvent.click(within(header).getByRole("button"));
    await waitFor(() => expect(header).toHaveAttribute("aria-sort", "descending"));
    await expect(modelOrder(canvasElement)).toEqual(["dear", "middling", "cheap"]);
  },
};

/** #2097: typing narrows the catalog by model name, and a miss says so */
export const FiltersByModelName: Story = {
  render: () => (
    <Harness fetchStub={withCurrency(CONFIGURED)}>
      <Pricing />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await canvas.findByText("gpt-4o");
    const search = canvas.getByRole("searchbox", { name: "Search models" });
    await userEvent.type(search, "claude");
    await waitFor(() => expect(canvas.queryByText("gpt-4o")).toBeNull());
    await expect(canvas.getByText("claude-sonnet")).toBeInTheDocument();
    await userEvent.clear(search);
    await userEvent.type(search, "nothing-like-this");
    await expect(await canvas.findByText("No prices match")).toBeVisible();
    await userEvent.click(canvas.getByRole("button", { name: "Clear filters" }));
    await expect(await canvas.findByText("gpt-4o")).toBeInTheDocument();
  },
};

/**
 * #2102: models that carried traffic with no price row are listed with their
 * request count, and "Add price" opens the sheet with the model filled in. A
 * model that already has a price is not listed, however the window saw it.
 */
export const ListsModelsWithUnpricedTraffic: Story = {
  render: () => (
    <Harness
      fetchStub={withCurrency(CONFIGURED, PRICES, [
        traffic("llama-3-70b", 1280),
        traffic("qwen-max", 42),
        // priced since: the window saw it unpriced, the catalog now has it
        traffic("gpt-4o", 7),
        traffic("claude-sonnet", 0),
      ])}
    >
      <Pricing />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expectListTable(canvasElement, "Models with unpriced traffic");
    const list = within(await canvas.findByRole("table", { name: "Models with unpriced traffic" }));
    await expect(list.getByText("llama-3-70b")).toBeInTheDocument();
    await expect(list.getByText("1,280")).toBeInTheDocument();
    await expect(list.getByText("qwen-max")).toBeInTheDocument();
    await expect(list.queryByText("gpt-4o")).toBeNull();
    await expect(
      canvas.getByRole("heading", { name: "2 models have traffic and no price" }),
    ).toBeVisible();
    await clickWhenEnabled(canvasElement, "Add a price for qwen-max");
    const form = within(sheet());
    await waitFor(() =>
      expect(form.getByRole("combobox", { name: "Model name" })).toHaveValue("qwen-max"),
    );
    await expect(form.getByLabelText("Input price per Mtok")).toHaveValue(null);
  },
};

/**
 * #3486: the traffic read answers one row per organization's model. A price is
 * per name, so two organizations' unpriced `gpt-4o` is one model to price, with
 * their requests added up.
 */
export const TwoOrganizationsUnpricedModelIsOneRow: Story = {
  render: () => (
    <Harness
      fetchStub={withCurrency(
        CONFIGURED,
        [],
        [
          traffic("gpt-4o", 30, "org-1"),
          traffic("gpt-4o", 12, "org-2"),
          traffic("qwen-max", 5, "org-1"),
        ],
      )}
    >
      <Pricing />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const list = within(await canvas.findByRole("table", { name: "Models with unpriced traffic" }));
    await expect(list.getAllByText("gpt-4o")).toHaveLength(1);
    await expect(list.getByText("42")).toBeInTheDocument();
    await expect(
      canvas.getByRole("heading", { name: "2 models have traffic and no price" }),
    ).toBeVisible();
  },
};

/** #2102: with every model priced the list is not drawn at all */
export const NoUnpricedTraffic: Story = {
  render: () => (
    <Harness
      fetchStub={withCurrency(CONFIGURED, PRICES, [traffic("gpt-4o"), traffic("mistral-large")])}
    >
      <Pricing />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await canvas.findByText("gpt-4o");
    await expect(canvas.queryByText(/have traffic and no price/)).toBeNull();
    await expect(canvas.queryByText(/Unpriced traffic needs ClickHouse/)).toBeNull();
  },
};

/**
 * #2102: a deployment with no analytics store cannot name its unpriced models,
 * and says so rather than leaving an absent list to read as "nothing missing".
 * The catalog below still works.
 */
export const UnpricedTrafficNeedsAnalytics: Story = {
  render: () => (
    <Harness
      fetchStub={scoped(async (input, init) =>
        String(input).includes("/analytics/")
          ? json({ error: { message: "no clickhouse_url" } }, 503)
          : withCurrency(CONFIGURED)(input, init),
      )}
    >
      <Pricing />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const panel = await expectAnalyticsUnavailable(
      canvasElement,
      "Unpriced traffic needs ClickHouse",
      "no clickhouse_url",
    );
    await expect(panel).toHaveTextContent(/Models can still be priced below/);
    await expect(await within(canvasElement).findByText("gpt-4o")).toBeInTheDocument();
  },
};

/** #2102: any other failure of the traffic read is an alert, with a retry */
export const UnpricedTrafficReadFails: Story = {
  render: () => (
    <Harness
      fetchStub={scoped(async (input, init) =>
        String(input).includes("/analytics/")
          ? json({ error: { message: "boom" } }, 500)
          : withCurrency(CONFIGURED)(input, init),
      )}
    >
      <Pricing />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectLoadError(canvasElement, /failed to return unpriced models/i);
    await expect(await within(canvasElement).findByText("gpt-4o")).toBeInTheDocument();
  },
};

/**
 * A price that sets a cache-write rate shows it in that column, and one that
 * does not shows a dash: with no rate, written tokens are priced at the input
 * rate, which is nothing worth a figure (#2876).
 */
export const ShowsTheCacheWriteRate: Story = {
  render: () => (
    <Harness fetchStub={withCurrency(CONFIGURED)}>
      <Pricing />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await canvas.findByText("claude-sonnet");
    await expect(canvas.getAllByText("3.75")).toHaveLength(1);
    const row = await rowOf(canvasElement, "claude-sonnet");
    await expect(row.getByText("3.75")).toBeInTheDocument();
  },
};

/**
 * The 1 hour write rate has a column of its own, told apart from the 5 minute
 * one by its header, and a row that sets neither shows a dash in both (#2902).
 */
export const ShowsTheOneHourCacheWriteRate: Story = {
  render: () => (
    <Harness fetchStub={withCurrency(CONFIGURED, SPLIT_PRICES)}>
      <Pricing />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await canvas.findByText("claude-opus");
    const columns = canvas.getAllByRole("columnheader").map((h) => h.textContent);
    const five = columns.indexOf("Cache write (5 min)");
    const hour = columns.indexOf("Cache write (1 hour)");
    await expect(five).toBeGreaterThan(0);
    await expect(hour).toBe(five + 1);
    const cell = async (model: string, index: number) => {
      const row = (await canvas.findByText(model)).closest<HTMLElement>('[role="row"]')!;
      return within(row).getAllByRole("cell")[index].textContent;
    };
    await expect(await cell("claude-opus", five)).toBe("3.75");
    await expect(await cell("claude-opus", hour)).toBe("6.00");
    // a price with only a 1 hour rate leaves the 5 minute column bare
    await expect(await cell("claude-haiku", five)).toBe("—");
    await expect(await cell("claude-haiku", hour)).toBe("2.00");
    // and a price that never set the 1 hour rate does not claim one
    await expect(await cell("claude-sonnet", hour)).toBe("—");
  },
};

export const Loading: Story = {
  render: () => (
    <Harness fetchStub={pending}>
      <Pricing />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectSkeleton(canvasElement);
    await expectNoFalseEmpty(canvasElement, /No model prices set/);
  },
};

export const Empty: Story = {
  render: () => (
    <Harness fetchStub={withCurrency(CONFIGURED, [])}>
      <Pricing />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    // the placeholder now says what an unpriced model costs the operator, and
    // carries the control that fixes it
    await expectEmptyState(canvasElement, /No model prices set/, /Add price/);
  },
};

export const Error_: Story = {
  name: "Error",
  render: () => (
    <Harness fetchStub={scoped(async () => json({ error: { message: "boom" } }, 500))}>
      <Pricing />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectLoadError(canvasElement, /failed to return model prices/i);
    await expectNoFalseEmpty(canvasElement, /No model prices set/);
  },
};

export const Forbidden: Story = {
  render: () => (
    <Harness fetchStub={scoped(async () => json({ error: { message: "forbidden" } }, 403))}>
      <Pricing />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectLoadError(canvasElement, /You do not have access to model prices/);
    await expectNoFalseEmpty(canvasElement, /No model prices set/);
  },
};

/**
 * #965: a price stored in a code the rate table no longer carries.
 *
 * It must still display — dropping it would hide real pricing — but its spend
 * is missing from base-currency totals, so it is flagged rather than silently
 * read as USD.
 */
export const UnconvertibleCurrency: Story = {
  render: () => (
    <Harness fetchStub={withCurrency(NARROWED)}>
      <Pricing />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    // the EUR and RUB prices are still listed...
    await expect(await canvas.findByText("mistral-large")).toBeInTheDocument();
    await expect(canvas.getByText("yandex-gpt")).toBeInTheDocument();
    // ...and both are called out as unconvertible, while the USD one is not
    await expect(await canvas.findAllByText(/no conversion rate/i)).toHaveLength(2);
  },
};

/**
 * A new price opens with empty rates (#2100): a zero default would turn an
 * unpriced model into a free one, and drop it from every unpriced flag.
 */
export const AddsAPrice: Story = {
  render: () => (
    <Harness fetchStub={withCurrency(CONFIGURED)}>
      <Pricing />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await clickWhenEnabled(canvasElement, /add price/i);
    const form = within(sheet());
    await expect(form.getByLabelText("Currency")).toHaveValue("USD");
    await expect(form.getByLabelText("Input price per Mtok")).toHaveValue(null);
    await expect(form.getByLabelText("Output price per Mtok")).toHaveValue(null);
    // the optional rates start empty too, and say what empty means
    const write = form.getByLabelText("5 minute cache-write price per Mtok (optional)");
    await expect(write).toHaveValue(null);
    await expect(write).toHaveAttribute("placeholder", "defaults to input price");
    // the 1 hour rate is a field of its own, named apart from the 5 minute one,
    // and says it falls back to that rate rather than to the input rate (#2902)
    const hour = form.getByLabelText("1 hour cache-write price per Mtok (optional)");
    await expect(hour).not.toBe(write);
    await expect(hour).toHaveValue(null);
    await expect(hour).toHaveAttribute("placeholder", "defaults to 5 minute price");
  },
};

/**
 * The Savings advisor flags a model with no known price and sends the operator
 * here with `?model=` (#3422). A model with no price row opens the price form
 * already named for it, for a role that can write prices; the link is then
 * taken out of the address so closing the form does not bring it back.
 */
export const AnUnpricedModelLinkOpensThePriceFormNamedForIt: Story = {
  render: () => (
    <Harness
      role="superadmin"
      route="/pricing-overrides?model=mystery-1"
      fetchStub={withCurrency(CONFIGURED)}
    >
      <Pricing />
      <AddressProbe />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const form = within(await within(document.body).findByRole("dialog"));
    await expect(await form.findByLabelText("Model name")).toHaveValue("mystery-1");
    // a new price: no rate is filled in for it
    await expect(form.getByLabelText("Input price per Mtok")).toHaveValue(null);
    await waitFor(() => expect(addressOf(canvasElement).has("model")).toBe(false));
  },
};

/**
 * A model that already has a price row narrows the list to it, and opens
 * nothing: the row's own Edit is one click away.
 */
export const APricedModelLinkNarrowsTheListToItsRow: Story = {
  render: () => (
    <Harness
      role="superadmin"
      route="/pricing-overrides?model=gpt-4o"
      fetchStub={withCurrency(CONFIGURED)}
    >
      <Pricing />
      <AddressProbe />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    // the link is read once the list has loaded, a step after the first paint
    await waitFor(() => expect(canvas.getByRole("searchbox")).toHaveValue("gpt-4o"));
    await waitFor(() => expect(modelOrder(canvasElement)).toEqual(["gpt-4o"]));
    await expect(canvas.queryByText("mistral-large")).toBeNull();
    await expect(document.querySelector('[role="dialog"]')).toBeNull();
    await expectAllowed(canvasElement, /Edit the price for gpt-4o/i);
    await waitFor(() => expect(addressOf(canvasElement).has("model")).toBe(false));
  },
};

/**
 * A role that cannot write prices follows the same link to the read-only list:
 * a priced model narrows it to its row, whose Edit is refused and names the
 * role it needs, and a model with no price opens no form at all.
 */
export const ALinkForARoleWithoutPriceEditingShowsTheReadOnlyRow: Story = {
  render: () => (
    <Harness
      role="admin"
      route="/pricing-overrides?model=gpt-4o"
      fetchStub={withCurrency(CONFIGURED)}
    >
      <Pricing />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(modelOrder(canvasElement)).toEqual(["gpt-4o"]));
    await expect(canvas.getByRole("searchbox")).toHaveValue("gpt-4o");
    await expectRefused(canvasElement, /Edit the price for gpt-4o/i, NEEDS_SUPERADMIN);
    await expect(document.querySelector('[role="dialog"]')).toBeNull();
  },
};

export const AnUnpricedModelLinkOpensNoFormForARoleWithoutPriceEditing: Story = {
  render: () => (
    <Harness
      role="admin"
      route="/pricing-overrides?model=mystery-1"
      fetchStub={withCurrency(CONFIGURED)}
    >
      <Pricing />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    // no price to read and none to write: the list says nothing matched
    await expect(await canvas.findByText(en.pages.pricing.noMatchTitle)).toBeVisible();
    await expect(canvas.getByRole("searchbox")).toHaveValue("mystery-1");
    await expect(document.querySelector('[role="dialog"]')).toBeNull();
    await expectRefused(canvasElement, /add price/i, NEEDS_SUPERADMIN);
  },
};

/** saving the untouched form is refused at each field, and sends nothing */
let emptySubmit: Recorder;
export const EmptySubmitIsRefused: Story = {
  render: () => {
    emptySubmit = recording(withCurrency(CONFIGURED));
    return (
      <Harness fetchStub={emptySubmit.stub}>
        <Pricing />
      </Harness>
    );
  },
  play: async ({ canvasElement }) => {
    await clickWhenEnabled(canvasElement, /add price/i);
    const form = within(sheet());
    // a pristine form is not flagged: the errors wait for the first refused save
    await expect(form.getByLabelText("Model name")).toBeValid();
    await expect(form.getByLabelText("Input price per Mtok")).toBeValid();
    await expect(form.queryByText("Choose a model to price.")).toBeNull();
    // the mandatory ones are announced up front without the native `required`,
    // which would flag the pristine form (#3451)
    for (const name of ["Model name", "Input price per Mtok", "Output price per Mtok"]) {
      const field = form.getByLabelText(name);
      await expect(field).toHaveAttribute("aria-required", "true");
      await expect(field).not.toHaveAttribute("required");
    }
    await expect(form.getByLabelText("Cached input price per Mtok (optional)")).not.toBeRequired();
    await expect(form.getAllByText("*")).toHaveLength(3);
    await userEvent.click(form.getByRole("button", { name: "Save" }));

    await expect(form.getByLabelText("Model name")).toBeInvalid();
    await expect(form.getByText("Choose a model to price.")).toBeInTheDocument();
    const input = form.getByLabelText("Input price per Mtok");
    await expect(input).toBeInvalid();
    await expect(input).toHaveAccessibleDescription(/Enter a price/);
    await expect(form.getByLabelText("Output price per Mtok")).toBeInvalid();
    await expect(form.getByLabelText("Cached input price per Mtok (optional)")).toBeValid();
    await expect(form.getByLabelText("5 minute cache-write price per Mtok (optional)")).toBeValid();
    await expect(form.getByLabelText("1 hour cache-write price per Mtok (optional)")).toBeValid();
    emptySubmit.expectNotSent("PUT", "/model-prices");

    // zero is allowed, but has to be typed
    await userEvent.type(input, "0");
    await expect(input).toBeValid();
  },
};

/** the model and currency are picked from what the screen knows, or typed */
let validSubmit: Recorder;
export const PicksAModelAndACurrency: Story = {
  render: () => {
    validSubmit = recording(
      scoped(async (input, init) =>
        init?.method === "PUT"
          ? json(price("llama-3-70b", "EUR"))
          : withCurrency(CONFIGURED)(input, init),
      ),
    );
    return (
      <Harness fetchStub={validSubmit.stub}>
        <Toasted>
          <Pricing />
        </Toasted>
      </Harness>
    );
  },
  play: async ({ canvasElement }) => {
    await clickWhenEnabled(canvasElement, /add price/i);
    const form = within(sheet());

    // routes first, then models seen in traffic that are not already routes
    const models = within(await openOptions(form.getByLabelText("Model name")));
    await waitFor(() =>
      expect(models.getAllByRole("option").map((o) => o.textContent)).toEqual([
        "gpt-4o",
        "claude-sonnet",
        "llama-3-70b",
      ]),
    );
    await userEvent.click(models.getByRole("option", { name: "llama-3-70b" }));
    await expect(form.getByLabelText("Model name")).toHaveValue("llama-3-70b");

    // the base currency and every code with a rate
    const currencies = within(await openOptions(form.getByLabelText("Currency")));
    await expect(currencies.getAllByRole("option").map((o) => o.textContent)).toEqual([
      "USDbase currency",
      "EUR",
      "RUB",
    ]);
    await userEvent.click(currencies.getByRole("option", { name: "EUR" }));

    await userEvent.type(form.getByLabelText("Input price per Mtok"), "0.59");
    await userEvent.type(form.getByLabelText("Output price per Mtok"), "0");
    await userEvent.click(form.getByRole("button", { name: "Save" }));

    await expect(
      await validSubmit.expectSentBody<Record<string, unknown>>("PUT", "/model-prices"),
    ).toEqual({
      model: "llama-3-70b",
      input_per_mtok: "0.59",
      output_per_mtok: "0",
      currency: "EUR",
    });
  },
};

/**
 * The Pricing screen's PUT, answered and recorded. The cache-write rate is the
 * one the control plane does not replace on every save (absent keeps it, `null`
 * clears it), so each story below reads what the form put on the wire.
 */
function pricesWithSave(prices = PRICES): Recorder {
  return recording(
    scoped(async (input, init) =>
      init?.method === "PUT"
        ? json(price("claude-sonnet", "USD", "claude-sonnet", "3.750000"))
        : withCurrency(CONFIGURED, prices)(input, init),
    ),
  );
}

/** a rate typed into a price that has none is sent, as the text that was typed */
let setsWriteRate: Recorder;
export const SetsTheCacheWriteRate: Story = {
  render: () => {
    setsWriteRate = pricesWithSave();
    return (
      <Harness fetchStub={setsWriteRate.stub}>
        <Toasted>
          <Pricing />
        </Toasted>
      </Harness>
    );
  },
  play: async ({ canvasElement }) => {
    await clickWhenEnabled(canvasElement, "Edit the price for gpt-4o");
    const form = within(sheet());
    const write = await form.findByLabelText("5 minute cache-write price per Mtok (optional)");
    await expect(write).toHaveValue(null);
    // the model is fixed and prefilled while editing, so only the prices are marked
    await expect(form.getByLabelText("Model name")).not.toHaveAttribute("aria-required");
    await expect(form.getByLabelText("Input price per Mtok")).toHaveAttribute(
      "aria-required",
      "true",
    );
    await userEvent.type(write, "3.75");
    await userEvent.click(form.getByRole("button", { name: "Save" }));

    await expect(
      await setsWriteRate.expectSentBody<Record<string, unknown>>("PUT", "/model-prices"),
    ).toEqual({
      model: "gpt-4o",
      input_per_mtok: "2.50",
      output_per_mtok: "10.00",
      cached_input_per_mtok: "1.25",
      cache_write_per_mtok: "3.75",
      currency: "USD",
    });
    await expectSheetClosed();
  },
};

/**
 * Editing something else leaves the stored rate alone: the form opens on the
 * rate, and a save that never touched it does not name it, so the control plane
 * keeps what it holds (#2876). Naming it would reset a rate changed through the
 * API or a config import since the card was drawn.
 */
let keepsWriteRate: Recorder;
export const LeavesTheCacheWriteRateAloneWhenUntouched: Story = {
  render: () => {
    keepsWriteRate = pricesWithSave();
    return (
      <Harness fetchStub={keepsWriteRate.stub}>
        <Toasted>
          <Pricing />
        </Toasted>
      </Harness>
    );
  },
  play: async ({ canvasElement }) => {
    await clickWhenEnabled(canvasElement, "Edit the price for claude-sonnet");
    const form = within(sheet());
    // a number input reads back numeric, so the six-place text shows as typed by a person
    await expect(
      await form.findByLabelText("5 minute cache-write price per Mtok (optional)"),
    ).toHaveValue(3.75);
    const output = form.getByLabelText("Output price per Mtok");
    await userEvent.clear(output);
    await userEvent.type(output, "12");
    await userEvent.click(form.getByRole("button", { name: "Save" }));

    const body = await keepsWriteRate.expectSentBody<Record<string, unknown>>(
      "PUT",
      "/model-prices",
    );
    await expect(body).toEqual({
      model: "claude-sonnet",
      input_per_mtok: "2.50",
      output_per_mtok: "12",
      cached_input_per_mtok: "1.25",
      currency: "USD",
    });
    await expect(body).not.toHaveProperty("cache_write_per_mtok");
  },
};

/**
 * Emptying the input is how the operator goes back to pricing writes at the
 * input rate, and it is sent as `null`: leaving the key out would keep the rate.
 */
let clearsWriteRate: Recorder;
export const ClearsTheCacheWriteRate: Story = {
  render: () => {
    clearsWriteRate = pricesWithSave();
    return (
      <Harness fetchStub={clearsWriteRate.stub}>
        <Toasted>
          <Pricing />
        </Toasted>
      </Harness>
    );
  },
  play: async ({ canvasElement }) => {
    await clickWhenEnabled(canvasElement, "Edit the price for claude-sonnet");
    const form = within(sheet());
    const write = await form.findByLabelText("5 minute cache-write price per Mtok (optional)");
    await expect(write).toHaveValue(3.75);
    await userEvent.clear(write);
    await userEvent.click(form.getByRole("button", { name: "Save" }));

    const body = await clearsWriteRate.expectSentBody<Record<string, unknown>>(
      "PUT",
      "/model-prices",
    );
    await expect(body).toHaveProperty("cache_write_per_mtok", null);
    await expectSheetClosed();
  },
};

/**
 * A rate that is not a number of 0 or more is refused at its own field after a
 * save is pressed, not before, and nothing is sent; fixing it lets the same
 * press through. The other prices say the same thing in the same words.
 */
let refusesWriteRate: Recorder;
export const RefusesAnInvalidCacheWriteRate: Story = {
  render: () => {
    refusesWriteRate = pricesWithSave();
    return (
      <Harness fetchStub={refusesWriteRate.stub}>
        <Toasted>
          <Pricing />
        </Toasted>
      </Harness>
    );
  },
  play: async ({ canvasElement }) => {
    await clickWhenEnabled(canvasElement, "Edit the price for gpt-4o");
    const form = within(sheet());
    const write = await form.findByLabelText("5 minute cache-write price per Mtok (optional)");
    await userEvent.type(write, "-3");
    // typing alone says nothing: the message waits for a refused save
    await expect(write).not.toHaveAttribute("aria-invalid");
    await expect(form.queryByText("Enter a price of 0 or more.")).toBeNull();

    await userEvent.click(form.getByRole("button", { name: "Save" }));
    await expect(write).toHaveAttribute("aria-invalid", "true");
    await expect(write).toHaveAccessibleDescription("Enter a price of 0 or more.");
    refusesWriteRate.expectNotSent("PUT", "/model-prices");
    // only its own field is flagged
    await expect(form.getByLabelText("Input price per Mtok")).toBeValid();
    await expect(form.getByLabelText("Cached input price per Mtok (optional)")).toBeValid();

    await userEvent.clear(write);
    await userEvent.type(write, "0");
    await expect(write).toBeValid();
    await userEvent.click(form.getByRole("button", { name: "Save" }));
    const body = await refusesWriteRate.expectSentBody<Record<string, unknown>>(
      "PUT",
      "/model-prices",
    );
    // zero is a rate: writes that cost nothing, not writes at the input rate
    await expect(body).toHaveProperty("cache_write_per_mtok", "0");
  },
};

const WRITE_5M = "5 minute cache-write price per Mtok (optional)";
const WRITE_1H = "1 hour cache-write price per Mtok (optional)";

/** a 1 hour rate typed into a price that has none is sent, and the 5 minute rate is not named */
let setsHourRate: Recorder;
export const SetsTheOneHourCacheWriteRate: Story = {
  render: () => {
    setsHourRate = pricesWithSave();
    return (
      <Harness fetchStub={setsHourRate.stub}>
        <Toasted>
          <Pricing />
        </Toasted>
      </Harness>
    );
  },
  play: async ({ canvasElement }) => {
    await clickWhenEnabled(canvasElement, "Edit the price for gpt-4o");
    const form = within(sheet());
    const hour = await form.findByLabelText(WRITE_1H);
    await expect(hour).toHaveValue(null);
    await userEvent.type(hour, "6");
    await userEvent.click(form.getByRole("button", { name: "Save" }));

    await expect(
      await setsHourRate.expectSentBody<Record<string, unknown>>("PUT", "/model-prices"),
    ).toEqual({
      model: "gpt-4o",
      input_per_mtok: "2.50",
      output_per_mtok: "10.00",
      cached_input_per_mtok: "1.25",
      cache_write_1h_per_mtok: "6",
      currency: "USD",
    });
    await expectSheetClosed();
  },
};

/**
 * The form opens on both rates, and a save that touched neither names neither,
 * so the control plane keeps what it holds: a 1 hour rate set through the API
 * since the card was drawn survives an edit of something else (#2902).
 */
let keepsHourRate: Recorder;
export const LeavesTheOneHourCacheWriteRateAloneWhenUntouched: Story = {
  render: () => {
    keepsHourRate = pricesWithSave(SPLIT_PRICES);
    return (
      <Harness fetchStub={keepsHourRate.stub}>
        <Toasted>
          <Pricing />
        </Toasted>
      </Harness>
    );
  },
  play: async ({ canvasElement }) => {
    await clickWhenEnabled(canvasElement, "Edit the price for claude-opus");
    const form = within(sheet());
    // a number input reads back numeric, so the six-place text shows as typed by a person
    await expect(await form.findByLabelText(WRITE_1H)).toHaveValue(6);
    await expect(form.getByLabelText(WRITE_5M)).toHaveValue(3.75);
    const output = form.getByLabelText("Output price per Mtok");
    await userEvent.clear(output);
    await userEvent.type(output, "12");
    await userEvent.click(form.getByRole("button", { name: "Save" }));

    const body = await keepsHourRate.expectSentBody<Record<string, unknown>>(
      "PUT",
      "/model-prices",
    );
    await expect(body).toEqual({
      model: "claude-opus",
      input_per_mtok: "2.50",
      output_per_mtok: "12",
      cached_input_per_mtok: "1.25",
      currency: "USD",
    });
    await expect(body).not.toHaveProperty("cache_write_1h_per_mtok");
    await expect(body).not.toHaveProperty("cache_write_per_mtok");
  },
};

/**
 * Emptying the 1 hour input sends `null`, which falls back to the 5 minute rate;
 * leaving the key out would keep the rate. Each input has its own key, so
 * emptying one and editing the other says both, each under its own name.
 */
let clearsHourRate: Recorder;
export const ClearsTheOneHourCacheWriteRate: Story = {
  render: () => {
    clearsHourRate = pricesWithSave(SPLIT_PRICES);
    return (
      <Harness fetchStub={clearsHourRate.stub}>
        <Toasted>
          <Pricing />
        </Toasted>
      </Harness>
    );
  },
  play: async ({ canvasElement }) => {
    await clickWhenEnabled(canvasElement, "Edit the price for claude-opus");
    const form = within(sheet());
    const hour = await form.findByLabelText(WRITE_1H);
    await expect(hour).toHaveValue(6);
    await userEvent.clear(hour);
    await userEvent.click(form.getByRole("button", { name: "Save" }));

    const body = await clearsHourRate.expectSentBody<Record<string, unknown>>(
      "PUT",
      "/model-prices",
    );
    await expect(body).toHaveProperty("cache_write_1h_per_mtok", null);
    await expect(body).not.toHaveProperty("cache_write_per_mtok");
    await expectSheetClosed();
  },
};

let editsBothRates: Recorder;
export const SendsEachCacheWriteRateUnderItsOwnKey: Story = {
  render: () => {
    editsBothRates = pricesWithSave(SPLIT_PRICES);
    return (
      <Harness fetchStub={editsBothRates.stub}>
        <Toasted>
          <Pricing />
        </Toasted>
      </Harness>
    );
  },
  play: async ({ canvasElement }) => {
    await clickWhenEnabled(canvasElement, "Edit the price for claude-opus");
    const form = within(sheet());
    const five = await form.findByLabelText(WRITE_5M);
    await userEvent.clear(five);
    const hour = form.getByLabelText(WRITE_1H);
    await userEvent.clear(hour);
    await userEvent.type(hour, "7.5");
    await userEvent.click(form.getByRole("button", { name: "Save" }));

    const body = await editsBothRates.expectSentBody<Record<string, unknown>>(
      "PUT",
      "/model-prices",
    );
    await expect(body).toHaveProperty("cache_write_per_mtok", null);
    await expect(body).toHaveProperty("cache_write_1h_per_mtok", "7.5");
  },
};

/**
 * Same rule as every price: a 1 hour rate that is not a number of 0 or more is
 * refused at its own input after a save is pressed, and only that input is
 * flagged; zero is a rate (#2902).
 */
let refusesHourRate: Recorder;
export const RefusesAnInvalidOneHourCacheWriteRate: Story = {
  render: () => {
    refusesHourRate = pricesWithSave();
    return (
      <Harness fetchStub={refusesHourRate.stub}>
        <Toasted>
          <Pricing />
        </Toasted>
      </Harness>
    );
  },
  play: async ({ canvasElement }) => {
    await clickWhenEnabled(canvasElement, "Edit the price for gpt-4o");
    const form = within(sheet());
    const hour = await form.findByLabelText(WRITE_1H);
    await userEvent.type(hour, "-2");
    await expect(hour).not.toHaveAttribute("aria-invalid");

    await userEvent.click(form.getByRole("button", { name: "Save" }));
    await expect(hour).toHaveAttribute("aria-invalid", "true");
    await expect(hour).toHaveAccessibleDescription("Enter a price of 0 or more.");
    refusesHourRate.expectNotSent("PUT", "/model-prices");
    await expect(form.getByLabelText(WRITE_5M)).toBeValid();

    await userEvent.clear(hour);
    await userEvent.type(hour, "0");
    await expect(hour).toBeValid();
    await userEvent.click(form.getByRole("button", { name: "Save" }));
    const body = await refusesHourRate.expectSentBody<Record<string, unknown>>(
      "PUT",
      "/model-prices",
    );
    await expect(body).toHaveProperty("cache_write_1h_per_mtok", "0");
    await expect(body).not.toHaveProperty("cache_write_per_mtok");
  },
};

/**
 * A rate the form lets through and the control plane refuses (one the stored
 * column cannot hold) is placed on the input the `400` names. The sheet stays
 * open with focus on that input, no toast repeats it, and editing the input
 * retires the message (#2902).
 */
const refusedRate = (field: string) =>
  recording(
    scoped(async (input, init) =>
      init?.method === "PUT"
        ? json(
            {
              error: {
                message: `${field} must be a number from 0 up to 999999.999999`,
                code: "invalid_field",
                field,
              },
            },
            400,
          )
        : withCurrency(CONFIGURED)(input, init),
    ),
  );

async function expectRefusalOnTheInput(
  canvasElement: HTMLElement,
  label: string,
  field: string,
): Promise<void> {
  await clickWhenEnabled(canvasElement, "Edit the price for gpt-4o");
  const form = within(sheet());
  const rate = await form.findByLabelText(label);
  await userEvent.type(rate, "1000000");
  // under the form's own limit, so only the control plane can say no
  await expect(rate).toBeValid();
  await userEvent.click(form.getByRole("button", { name: "Save" }));

  await waitFor(() => expect(rate).toHaveAttribute("aria-invalid", "true"));
  await expect(rate).toHaveAccessibleDescription(
    `${field} must be a number from 0 up to 999999.999999`,
  );
  await waitFor(() => expect(rate).toHaveFocus());
  await expect(sheet()).toBeVisible();
  // the refusal is at the input, not repeated as a toast or above the buttons
  await expect(within(document.body).queryByText(/Could not save/)).toBeNull();
  await expect(form.getAllByText(new RegExp(field))).toHaveLength(1);
  await userEvent.type(rate, "0");
  await waitFor(() => expect(rate).not.toHaveAttribute("aria-invalid"));
}

let refusedHour: Recorder;
export const PinsAnOneHourRateTheServerRefusesToItsInput: Story = {
  render: () => {
    refusedHour = refusedRate("cache_write_1h_per_mtok");
    return (
      <Harness fetchStub={refusedHour.stub}>
        <Toasted>
          <Pricing />
        </Toasted>
      </Harness>
    );
  },
  play: async ({ canvasElement }) => {
    await expectRefusalOnTheInput(canvasElement, WRITE_1H, "cache_write_1h_per_mtok");
    // only the input the error names is flagged
    await expect(within(sheet()).getByLabelText(WRITE_5M)).toBeValid();
  },
};

let refusedFiveMinute: Recorder;
export const PinsAFiveMinuteRateTheServerRefusesToItsInput: Story = {
  render: () => {
    refusedFiveMinute = refusedRate("cache_write_per_mtok");
    return (
      <Harness fetchStub={refusedFiveMinute.stub}>
        <Toasted>
          <Pricing />
        </Toasted>
      </Harness>
    );
  },
  play: async ({ canvasElement }) => {
    await expectRefusalOnTheInput(canvasElement, WRITE_5M, "cache_write_per_mtok");
    await expect(within(sheet()).getByLabelText(WRITE_1H)).toBeValid();
  },
};

/** a model or a code the lists do not carry can still be typed, with a warning */
export const AcceptsAFreeModelAndCode: Story = {
  render: () => (
    <Harness fetchStub={withCurrency(CONFIGURED)}>
      <Pricing />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await clickWhenEnabled(canvasElement, /add price/i);
    const form = within(sheet());
    await userEvent.type(form.getByLabelText("Model name"), "my-finetune");
    await userEvent.click(await form.findByRole("option", { name: /Use “my-finetune”/ }));
    await expect(form.getByLabelText("Model name")).toHaveValue("my-finetune");

    await userEvent.type(form.getByLabelText("Currency"), "KZT");
    await userEvent.click(await form.findByRole("option", { name: /Use “KZT”/ }));
    await expect(form.getByLabelText("Currency")).toHaveValue("KZT");
    await expect(form.getByLabelText("Currency")).toHaveAccessibleDescription(
      /KZT, which has no conversion rate/,
    );
  },
};

/**
 * The price is refused (#1607).
 *
 * The sheet closes on success, so a rejected save is the only case where it has
 * to stay — and it has to, because the three rates were typed by hand from a
 * provider's price sheet.
 */
export const AddRejectedByTheServer: Story = {
  render: () => (
    <Harness
      fetchStub={scoped(async (input, init) =>
        init?.method === "PUT"
          ? json({ error: { message: "gpt-4o already has a price in USD" } }, 409)
          : withCurrency(CONFIGURED)(input, init),
      )}
    >
      <Toasted>
        <Pricing />
      </Toasted>
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await clickWhenEnabled(canvasElement, /add price/i);
    const form = within(sheet());
    await pickOption(form.getByLabelText("Model name"), "gpt-4o");
    await userEvent.type(form.getByLabelText("Input price per Mtok"), "2.50");
    await userEvent.type(form.getByLabelText("Output price per Mtok"), "10.00");
    await userEvent.click(form.getByRole("button", { name: "Save" }));

    await expectToast(canvasElement, /already has a price in USD/, "error");
    await waitFor(() => expect(within(document.body).getByRole("dialog")).toBeInTheDocument());
    await expect(form.getByLabelText("Model name")).toHaveValue("gpt-4o");
    // a number input, so the value reads back numeric rather than as typed
    await expect(form.getByLabelText("Output price per Mtok")).toHaveValue(10);
  },
};

/**
 * Deleting a price confirms through the shared `ConfirmDialog` (#1760): the
 * title names the model, a cancel sends nothing and is an abandon, and a
 * confirm sends the DELETE and lands as `save_confirmed`.
 */
let priceDeletes: Recorder;
export const DeleteIsConfirmedAndReported: Story = {
  beforeEach: recordUxEvents,
  render: () => {
    priceDeletes = recording(
      scoped(async (input, init) =>
        init?.method === "DELETE" ? json(null, 204) : withCurrency(CONFIGURED)(input, init),
      ),
    );
    return (
      <Harness fetchStub={priceDeletes.stub}>
        <UxScreenProvider screen="pricing-overrides">
          <Pricing />
        </UxScreenProvider>
      </Harness>
    );
  },
  play: async ({ canvasElement }) => {
    await clickWhenEnabled(canvasElement, "Delete the price for mistral-large");
    await expect(
      await within(document.body).findByRole("heading", {
        name: "Delete the price for mistral-large?",
      }),
    ).toBeInTheDocument();
    await cancelConfirmation();
    priceDeletes.expectNotSent("DELETE", "/model-prices/");
    const abandon = await expectUxEvent("form_abandon", "price-delete");
    await expect(abandon.screen).toBe("pricing-overrides");
    await expect(abandon.outcome).toBe("cancelled");
    expectNoUxEvent("form_submit", "price-delete");

    await clickWhenEnabled(canvasElement, "Delete the price for mistral-large");
    await confirmDestructive(/mistral-large/, "Delete price");
    await priceDeletes.expectSent("DELETE", "/model-prices/mistral-large");
    await expectSheetClosed();
    const submit = await expectUxEvent("form_submit", "price-delete");
    await expect(submit.outcome).toBe("ok");
    await expectUxEvent("save_confirmed", "price-delete");
  },
};

/**
 * A refused delete keeps the dialog open with the control plane's reason, and
 * the refusal reaches the UX stream beside the press even though the stub
 * answers in the same tick (#1761).
 */
export const DeleteRejectedByTheServer: Story = {
  beforeEach: recordUxEvents,
  render: () => (
    <Harness
      fetchStub={scoped(async (input, init) =>
        init?.method === "DELETE"
          ? json({ error: { message: "the store is read-only while a restore runs" } }, 409)
          : withCurrency(CONFIGURED)(input, init),
      )}
    >
      <UxScreenProvider screen="pricing-overrides">
        <Toasted>
          <Pricing />
        </Toasted>
      </UxScreenProvider>
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await clickWhenEnabled(canvasElement, "Delete the price for gpt-4o");
    await confirmDestructive(/gpt-4o/, "Delete price");
    await waitFor(() =>
      expect(
        uxEvents()
          .filter((e) => e.action === "form_submit" && e.target === "price-delete")
          .map((e) => e.outcome),
      ).toEqual(["ok", "error"]),
    );
    expectNoUxEvent("save_confirmed", "price-delete");
    await expectToast(canvasElement, /read-only while a restore runs/, "error");
    const dialog = within(await within(document.body).findByRole("dialog"));
    await expect(dialog.getByRole("alert")).toHaveTextContent("read-only while a restore runs");
  },
};

const IMAGE = "Price per generated image (optional)";
const AUDIO = "Price per minute of input audio (optional)";
const SPEECH = "Price per million characters of speech input (optional)";

/** A price that sets unit rates shows them in their own columns (#2937). */
export const ShowsTheUnitRatesInTheirColumns: Story = {
  render: () => (
    <Harness fetchStub={withCurrency(CONFIGURED, MEDIA_PRICES)}>
      <Pricing />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(await canvas.findByText("Image")).toBeInTheDocument();
    await expect(canvas.getByText("Audio (per minute)")).toBeInTheDocument();
    await expect(canvas.getByText("Speech (per Mchar)")).toBeInTheDocument();
    await expect(canvas.getByText("0.04")).toBeInTheDocument();
    await expect(canvas.getByText("15.00")).toBeInTheDocument();
  },
};

/**
 * The form opens on the three unit rates and a save that touched none names
 * none, so the control plane keeps what it holds (#2937).
 */
let keepsUnitRates: Recorder;
export const LeavesTheUnitRatesAloneWhenUntouched: Story = {
  render: () => {
    keepsUnitRates = pricesWithSave(MEDIA_PRICES);
    return (
      <Harness fetchStub={keepsUnitRates.stub}>
        <Toasted>
          <Pricing />
        </Toasted>
      </Harness>
    );
  },
  play: async ({ canvasElement }) => {
    await clickWhenEnabled(canvasElement, "Edit the price for dall-e-3");
    const form = within(sheet());
    await expect(await form.findByLabelText(IMAGE)).toHaveValue(0.04);
    await expect(form.getByLabelText(AUDIO)).toHaveValue(0.006);
    await expect(form.getByLabelText(SPEECH)).toHaveValue(15);
    const input = form.getByLabelText("Input price per Mtok");
    await userEvent.clear(input);
    await userEvent.type(input, "1");
    await userEvent.click(form.getByRole("button", { name: "Save" }));

    const body = await keepsUnitRates.expectSentBody<Record<string, unknown>>(
      "PUT",
      "/model-prices",
    );
    await expect(body).not.toHaveProperty("image_per_unit");
    await expect(body).not.toHaveProperty("audio_input_per_minute");
    await expect(body).not.toHaveProperty("speech_per_mchar");
  },
};

/** Editing and emptying a unit input sends its own key: a value, or null to clear. */
let editsUnitRates: Recorder;
export const SendsEachUnitRateUnderItsOwnKey: Story = {
  render: () => {
    editsUnitRates = pricesWithSave(MEDIA_PRICES);
    return (
      <Harness fetchStub={editsUnitRates.stub}>
        <Toasted>
          <Pricing />
        </Toasted>
      </Harness>
    );
  },
  play: async ({ canvasElement }) => {
    await clickWhenEnabled(canvasElement, "Edit the price for dall-e-3");
    const form = within(sheet());
    const image = await form.findByLabelText(IMAGE);
    await userEvent.clear(image);
    await userEvent.type(image, "0.08");
    await userEvent.clear(form.getByLabelText(SPEECH));
    await userEvent.click(form.getByRole("button", { name: "Save" }));

    const body = await editsUnitRates.expectSentBody<Record<string, unknown>>(
      "PUT",
      "/model-prices",
    );
    await expect(body).toHaveProperty("image_per_unit", "0.08");
    await expect(body).toHaveProperty("speech_per_mchar", null);
    await expect(body).not.toHaveProperty("audio_input_per_minute");
  },
};

/** A unit rate that is not a number of 0 or more is refused at its own field. */
let refusesUnitRate: Recorder;
export const RefusesAnInvalidUnitRate: Story = {
  render: () => {
    refusesUnitRate = pricesWithSave();
    return (
      <Harness fetchStub={refusesUnitRate.stub}>
        <Toasted>
          <Pricing />
        </Toasted>
      </Harness>
    );
  },
  play: async ({ canvasElement }) => {
    await clickWhenEnabled(canvasElement, "Edit the price for gpt-4o");
    const form = within(sheet());
    const image = await form.findByLabelText(IMAGE);
    await userEvent.type(image, "-1");
    await userEvent.click(form.getByRole("button", { name: "Save" }));
    await expect(image).toHaveAttribute("aria-invalid", "true");
    await expect(image).toHaveAccessibleDescription("Enter a price of 0 or more.");
    refusesUnitRate.expectNotSent("PUT", "/model-prices");
    await expect(form.getByLabelText(AUDIO)).toBeValid();
  },
};

const AUDIO_PRICES: ModelPriceRow[] = [
  {
    ...price("gpt-realtime", "USD"),
    input_per_mtok: "4.000000",
    output_per_mtok: "16.000000",
    cached_input_per_mtok: "0.400000",
    audio_input_per_mtok: "32.000000",
    audio_output_per_mtok: "64.000000",
    cached_audio_input_per_mtok: null,
  },
];

const AUDIO_IN = "Audio input price per Mtok (optional)";
const AUDIO_OUT = "Audio output price per Mtok (optional)";
const CACHED_AUDIO = "Cached audio input price per Mtok (optional)";

/** A price that sets audio token rates shows them in their own columns (#1882). */
export const ShowsTheAudioTokenRatesInTheirColumns: Story = {
  render: () => (
    <Harness fetchStub={withCurrency(CONFIGURED, AUDIO_PRICES)}>
      <Pricing />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(await canvas.findByText("Audio input")).toBeInTheDocument();
    await expect(canvas.getByText("Audio output")).toBeInTheDocument();
    await expect(canvas.getByText("Cached audio input")).toBeInTheDocument();
    await expect(canvas.getByText("32.00")).toBeInTheDocument();
    await expect(canvas.getByText("64.00")).toBeInTheDocument();
  },
};

/**
 * The form opens on the audio token rates and a save that touched none names
 * none, so the control plane keeps what it holds (#1882).
 */
let keepsAudioRates: Recorder;
export const LeavesTheAudioTokenRatesAloneWhenUntouched: Story = {
  render: () => {
    keepsAudioRates = pricesWithSave(AUDIO_PRICES);
    return (
      <Harness fetchStub={keepsAudioRates.stub}>
        <Toasted>
          <Pricing />
        </Toasted>
      </Harness>
    );
  },
  play: async ({ canvasElement }) => {
    await clickWhenEnabled(canvasElement, "Edit the price for gpt-realtime");
    const form = within(sheet());
    await expect(await form.findByLabelText(AUDIO_IN)).toHaveValue(32);
    await expect(form.getByLabelText(AUDIO_OUT)).toHaveValue(64);
    await expect(form.getByLabelText(CACHED_AUDIO)).toHaveValue(null);
    const input = form.getByLabelText("Input price per Mtok");
    await userEvent.clear(input);
    await userEvent.type(input, "5");
    await userEvent.click(form.getByRole("button", { name: "Save" }));

    const body = await keepsAudioRates.expectSentBody<Record<string, unknown>>(
      "PUT",
      "/model-prices",
    );
    await expect(body).not.toHaveProperty("audio_input_per_mtok");
    await expect(body).not.toHaveProperty("audio_output_per_mtok");
    await expect(body).not.toHaveProperty("cached_audio_input_per_mtok");
  },
};

/** Editing and emptying an audio input sends its own key: a value, or null to clear. */
let editsAudioRates: Recorder;
export const SendsEachAudioTokenRateUnderItsOwnKey: Story = {
  render: () => {
    editsAudioRates = pricesWithSave(AUDIO_PRICES);
    return (
      <Harness fetchStub={editsAudioRates.stub}>
        <Toasted>
          <Pricing />
        </Toasted>
      </Harness>
    );
  },
  play: async ({ canvasElement }) => {
    await clickWhenEnabled(canvasElement, "Edit the price for gpt-realtime");
    const form = within(sheet());
    const audioIn = await form.findByLabelText(AUDIO_IN);
    await userEvent.clear(audioIn);
    await userEvent.type(audioIn, "40");
    await userEvent.clear(form.getByLabelText(AUDIO_OUT));
    await userEvent.type(form.getByLabelText(CACHED_AUDIO), "0.5");
    await userEvent.click(form.getByRole("button", { name: "Save" }));

    const body = await editsAudioRates.expectSentBody<Record<string, unknown>>(
      "PUT",
      "/model-prices",
    );
    await expect(body).toHaveProperty("audio_input_per_mtok", "40");
    await expect(body).toHaveProperty("audio_output_per_mtok", null);
    await expect(body).toHaveProperty("cached_audio_input_per_mtok", "0.5");
  },
};

/** An audio token rate that is not a number of 0 or more is refused at its own field. */
let refusesAudioRate: Recorder;
export const RefusesAnInvalidAudioTokenRate: Story = {
  render: () => {
    refusesAudioRate = pricesWithSave();
    return (
      <Harness fetchStub={refusesAudioRate.stub}>
        <Toasted>
          <Pricing />
        </Toasted>
      </Harness>
    );
  },
  play: async ({ canvasElement }) => {
    await clickWhenEnabled(canvasElement, "Edit the price for gpt-4o");
    const form = within(sheet());
    const audioIn = await form.findByLabelText(AUDIO_IN);
    await userEvent.type(audioIn, "-1");
    await userEvent.click(form.getByRole("button", { name: "Save" }));
    await expect(audioIn).toHaveAttribute("aria-invalid", "true");
    await expect(audioIn).toHaveAccessibleDescription("Enter a price of 0 or more.");
    refusesAudioRate.expectNotSent("PUT", "/model-prices");
    await expect(form.getByLabelText(AUDIO_OUT)).toBeValid();
  },
};

// --- catalogue import (#966) ---

/** a price imported from the vendored snapshot, and one an operator set */
const IMPORTED: ModelPriceRow[] = [
  {
    ...price("gpt-4o", "USD"),
    source: "LiteLLM model_prices_and_context_window",
    source_imported_at: "2026-10-01T00:00:00Z",
  },
  { ...price("mistral-large", "EUR"), source: null, source_imported_at: null },
];

const CATALOGUE: PriceCatalogue = {
  source: "LiteLLM model_prices_and_context_window",
  licence: "MIT",
  url: "https://github.com/BerriAI/litellm",
  currency: "USD",
  snapshot: "2026-10-01",
  total: 1200,
  entries: [
    {
      model: "gpt-4o",
      input_per_mtok: "2.500000",
      output_per_mtok: "10.000000",
      cached_input_per_mtok: "1.250000",
      cache_write_per_mtok: null,
      currency: "USD",
    },
    {
      model: "mistral-large",
      input_per_mtok: "2.000000",
      output_per_mtok: "6.000000",
      cached_input_per_mtok: null,
      cache_write_per_mtok: null,
      currency: "USD",
    },
    {
      model: "claude-sonnet",
      input_per_mtok: "3.000000",
      output_per_mtok: "15.000000",
      cached_input_per_mtok: "0.300000",
      cache_write_per_mtok: "3.750000",
      currency: "USD",
    },
  ],
};

/**
 * The screen with the import endpoints answered. The catalogue and import paths
 * share the `/model-prices` prefix with the list, so they are matched first.
 */
function importStub(
  prices: ModelPriceRow[],
  catalogue: (url: string) => Response | Promise<Response>,
  imported?: () => unknown,
) {
  return recording(
    scoped(async (input, init) => {
      const url = String(input);
      if (url.includes("/model-prices/catalogue")) return catalogue(url);
      if (url.includes("/model-prices/import")) return json(imported?.() ?? {});
      return withCurrency(CONFIGURED, prices)(input, init);
    }),
  );
}

/** a catalogue answer filtered by the `q` the dialog sent */
const searchable = (url: string) => {
  const q = (new URL(url, "http://x").searchParams.get("q") ?? "").toLowerCase();
  const entries = CATALOGUE.entries.filter((e) => e.model.includes(q));
  return json({ ...CATALOGUE, entries });
};

export const ShowsWhereEachPriceCameFrom: Story = {
  render: () => (
    <Harness fetchStub={withCurrency(CONFIGURED, IMPORTED)}>
      <Pricing />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const imported = await rowOf(canvasElement, "gpt-4o");
    const badge = await imported.findByText("Catalogue");
    // the badge states the source, the snapshot day and that it can change
    await expect(badge).toHaveAttribute("title", expect.stringContaining("LiteLLM"));
    await expect(badge).toHaveAttribute("title", expect.stringContaining("Oct 1, 2026"));
    await expect(badge).toHaveAttribute("title", expect.stringContaining("subject to change"));
    const own = await rowOf(canvasElement, "mistral-large");
    await expect(own.getByText("Override")).toBeInTheDocument();
    await expect(own.queryByText("Catalogue")).toBeNull();
  },
};

let importSearch: Recorder;
export const ImportSearchesTheCatalogue: Story = {
  render: () => {
    importSearch = importStub(IMPORTED, searchable);
    return (
      <Harness fetchStub={importSearch.stub}>
        <Pricing />
      </Harness>
    );
  },
  play: async ({ canvasElement }) => {
    await clickWhenEnabled(canvasElement, /import from catalogue/i);
    const dialog = within(sheet());
    // the notice, the source line and the snapshot date are on screen before anything is picked
    await waitFor(() =>
      expect(
        dialog.getByText(/Catalogue values are third-party list prices, subject to change/),
      ).toBeVisible(),
    );
    await expect(dialog.getByText(/the source is provisional/i)).toBeInTheDocument();
    await expect(
      await dialog.findByText(
        /LiteLLM model_prices_and_context_window \(MIT\) · snapshot Oct 1, 2026/,
      ),
    ).toBeInTheDocument();
    const list = await dialog.findByRole("list", { name: "Catalogue models" });
    await expect(within(list).getAllByRole("listitem")).toHaveLength(3);
    await expect(within(list).getByText(/input 2\.50 · output 10\.00/)).toBeInTheDocument();
    // the model the operator priced is marked as one the import keeps
    await expect(within(list).getByTitle(/an import keeps it/)).toBeInTheDocument();

    await userEvent.type(dialog.getByRole("searchbox"), "claude");
    await waitFor(() =>
      expect(
        within(dialog.getByRole("list", { name: "Catalogue models" })).getAllByRole("listitem"),
      ).toHaveLength(1),
    );
    await waitFor(() =>
      expect(importSearch.calls.some((c) => c.url.includes("q=claude"))).toBe(true),
    );
    // nothing is imported until somebody presses the button
    importSearch.expectNotSent("POST", "/model-prices/import");
    await expect(dialog.getByRole("button", { name: "Import selected (0)" })).toBeDisabled();
  },
};

let importRun: Recorder;
export const ImportReportsEachOutcome: Story = {
  render: () => {
    importRun = importStub(IMPORTED, searchable, () => ({
      source: CATALOGUE.source,
      currency: "USD",
      snapshot: "2026-10-01",
      results: [
        { model: "claude-sonnet", outcome: "created" },
        { model: "gpt-4o", outcome: "updated" },
        { model: "mistral-large", outcome: "kept_override" },
        { model: "no-such-model", outcome: "not_in_catalogue" },
      ],
    }));
    return (
      <Harness fetchStub={importRun.stub}>
        <Pricing />
      </Harness>
    );
  },
  play: async ({ canvasElement }) => {
    await clickWhenEnabled(canvasElement, /import from catalogue/i);
    const dialog = within(sheet());
    const list = await dialog.findByRole("list", { name: "Catalogue models" });
    for (const model of ["claude-sonnet", "gpt-4o", "mistral-large"]) {
      await userEvent.click(within(list).getByText(model));
    }
    await userEvent.click(dialog.getByRole("button", { name: "Import selected (3)" }));

    await expect(
      await importRun.expectSentBody<Record<string, unknown>>("POST", "/model-prices/import"),
    ).toEqual({ models: ["claude-sonnet", "gpt-4o", "mistral-large"] });
    const result = await dialog.findByRole("status");
    await expect(result).toHaveTextContent(/snapshot Oct 1, 2026/);
    await expect(result).toHaveTextContent(/subject to change/);
    await expect(result).toHaveTextContent("Created: claude-sonnet");
    await expect(result).toHaveTextContent("Refreshed from the catalogue: gpt-4o");
    await expect(result).toHaveTextContent("Kept your own price: mistral-large");
    await expect(result).toHaveTextContent("Not in the catalogue: no-such-model");
  },
};

export const ImportLoading: Story = {
  render: () => (
    <Harness fetchStub={importStub(IMPORTED, () => new Promise<Response>(() => {})).stub}>
      <Pricing />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await clickWhenEnabled(canvasElement, /import from catalogue/i);
    const dialog = sheet();
    await waitFor(() => expect(within(dialog).getAllByRole("status").length).toBeGreaterThan(0));
    await expect(within(dialog).queryByRole("list", { name: "Catalogue models" })).toBeNull();
  },
};

export const ImportCatalogueEmpty: Story = {
  render: () => (
    <Harness
      fetchStub={importStub(IMPORTED, () => json({ ...CATALOGUE, total: 0, entries: [] })).stub}
    >
      <Pricing />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await clickWhenEnabled(canvasElement, /import from catalogue/i);
    await expect(
      await within(sheet()).findByText("No catalogue model matches"),
    ).toBeInTheDocument();
  },
};

export const ImportCatalogueError: Story = {
  render: () => (
    <Harness fetchStub={importStub(IMPORTED, () => json({ error: { message: "boom" } }, 500)).stub}>
      <Pricing />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await clickWhenEnabled(canvasElement, /import from catalogue/i);
    await expectLoadError(sheet(), /failed to return price catalogue/i);
    await expect(within(sheet()).queryByText("No catalogue model matches")).toBeNull();
  },
};

export const ImportIsGated: Story = {
  render: () => (
    <Harness role="admin" fetchStub={withCurrency(CONFIGURED, IMPORTED)}>
      <Pricing />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectRefused(canvasElement, /import from catalogue/i, NEEDS_SUPERADMIN);
  },
};

export const ImportIsOpenToASuperadmin: Story = {
  render: () => (
    <Harness role="superadmin" fetchStub={withCurrency(CONFIGURED, IMPORTED)}>
      <Pricing />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectAllowed(canvasElement, /import from catalogue/i);
  },
};

/** The model name explains itself in an (i) (#3285). */
export const ExplainsTheModelNameInAHint: Story = {
  render: () => (
    <Harness fetchStub={withCurrency(CONFIGURED)}>
      <Pricing />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await clickWhenEnabled(canvasElement, /add price/i);
    const p = en.pages.pricing;
    await expectFieldHints(sheet(), en.common.aboutField, [[p.modelName, p.modelNameInfo]]);
  },
};
