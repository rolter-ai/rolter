import type { Meta, StoryObj } from "@storybook/react-vite";
import { expect, userEvent, waitFor, within } from "storybook/test";

import Pricing from "./Pricing";
import {
  Harness,
  cancelConfirmation,
  clickWhenEnabled,
  confirmDestructive,
  expectEmptyState,
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
} from "./story-harness";
import type { CurrencySettings, ModelPriceRow } from "@/lib/api";
import { UxScreenProvider } from "@/lib/ux-react";

const price = (model: string, currency: string, id = model): ModelPriceRow => ({
  id,
  model,
  input_per_mtok: "2.50",
  output_per_mtok: "10.00",
  cached_input_per_mtok: "1.25",
  currency,
  created_at: "2026-07-01T00:00:00Z",
});

const PRICES: ModelPriceRow[] = [
  price("gpt-4o", "USD"),
  price("mistral-large", "EUR"),
  // priced in a code this deployment's rate table carries — #965's whole point
  price("yandex-gpt", "RUB"),
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

const withCurrency = (settings: CurrencySettings, prices = PRICES) =>
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
    [
      "/api/v1/analytics/by-model",
      () => ({ data: [{ model: "gpt-4o" }, { model: "llama-3-70b" }] }),
    ],
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
    await userEvent.click(form.getByRole("button", { name: "Save" }));

    await expect(form.getByLabelText("Model name")).toBeInvalid();
    await expect(form.getByText("Choose a model to price.")).toBeInTheDocument();
    const input = form.getByLabelText("Input price per Mtok");
    await expect(input).toBeInvalid();
    await expect(input).toHaveAccessibleDescription(/Enter a price/);
    await expect(form.getByLabelText("Output price per Mtok")).toBeInvalid();
    await expect(form.getByLabelText("Cached input price per Mtok (optional)")).toBeValid();
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
