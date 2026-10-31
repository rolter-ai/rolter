import type { Meta, StoryObj } from "@storybook/react";
import { expect, userEvent, within } from "storybook/test";

import en from "@/lib/i18n/locales/en.json";
import {
  captureFollowedLinks,
  expectToast,
  Harness,
  json,
  recording,
  STORY_DOWNLOAD_URL,
  stubSavePicker,
  Toasted,
  withExportTickets,
  type Recorder,
} from "@/pages/story-harness";

import { ExportCsvButton } from "./ExportCsvButton";

const REQUEST = {
  path: "/api/v1/analytics/invocations/export",
  query: "status=error&since=2026-10-01T00%3A00%3A00.000Z",
};

const nothingElse = async () => json({ error: { message: "not stubbed" } }, 404);

function Button() {
  return (
    <ExportCsvButton
      what={en.errors.resources.requestLogExport}
      fallbackName="request-log.csv"
      toDisk
      request={() => REQUEST}
    />
  );
}

let calls: Recorder;
function story(reads: Record<string, unknown>[], mint?: () => Response) {
  return () => {
    calls = recording(withExportTickets(nothingElse, reads, mint));
    return (
      <Harness fetchStub={calls.stub}>
        <Toasted>
          <Button />
        </Toasted>
      </Harness>
    );
  };
}

/**
 * Press Export in a browser with no save dialog (Firefox, Safari, plain http),
 * and collect the links the page hands to the browser while `body` runs.
 */
async function exportWithoutSaveDialog(canvasElement: HTMLElement, body: () => Promise<void>) {
  const canvas = within(canvasElement);
  const links: string[] = [];
  await stubSavePicker().within(
    () =>
      captureFollowedLinks(links, async () => {
        await userEvent.click(await canvas.findByRole("button", { name: "Export CSV" }));
        await body();
      }),
    false,
  );
  return links;
}

const meta = {
  title: "Components/ExportCsvButton",
  component: Button,
} satisfies Meta<typeof Button>;

export default meta;
type Story = StoryObj<typeof meta>;

/**
 * #3504: without the File System Access API the export goes to the browser's
 * download manager, so the tab never holds it. The button mints a ticket for
 * the export on screen, follows its link once, and reads the ticket's status
 * for the rows written until the file is complete. The browser's own download
 * list is where that download is cancelled; the page offers only to stop
 * following it.
 */
export const ThroughTheBrowserItShowsTheRowsWritten: Story = {
  render: story([
    { state: "pending" },
    { state: "streaming", rows: 1000 },
    { state: "streaming", rows: 2500 },
    { state: "complete", rows: 3000 },
  ]),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const links = await exportWithoutSaveDialog(canvasElement, async () => {
      // the count the status reports, in the status region, while it runs
      const count = await canvas.findByText(/^[\d,]+ rows written$/);
      await expect(count).toHaveAttribute("role", "status");
      await expect(canvas.getByRole("button", { name: "Exporting…" })).toBeDisabled();
      // no Cancel of the request: the browser owns the download, so the one
      // control there only stops following it, and its name says so
      await expect(canvas.queryByRole("button", { name: /Cancel the/ })).toBeNull();
      await expect(
        canvas.getByRole("button", { name: /the browser's download carries on/ }),
      ).toBeVisible();
      await expectToast(canvasElement, /Export downloaded/, "success");
    });
    await expect(links).toEqual([STORY_DOWNLOAD_URL]);
    // the ticket names exactly the export on screen
    const body = await calls.expectSentBody<typeof REQUEST>(
      "POST",
      "/api/v1/analytics/exports/tickets",
    );
    await expect(body).toEqual(REQUEST);
    // the page fetches neither the export nor the download link itself
    await expect(calls.calls.some((c) => c.url.includes("/invocations/export"))).toBe(false);
    await expect(calls.calls.some((c) => c.url.includes("/exports/download/"))).toBe(false);
    await expect(await canvas.findByRole("button", { name: "Export CSV" })).toBeEnabled();
  },
};

/**
 * A full export gate refuses the mint with `429` and `Retry-After`, before the
 * browser is handed anything: the same words the bearer export's `429` gets.
 */
export const AFullGateRefusesTheTicket: Story = {
  render: story(
    [],
    () =>
      new Response(JSON.stringify({ error: { message: "you already have one running" } }), {
        status: 429,
        headers: { "content-type": "application/json", "retry-after": "15" },
      }),
  ),
  play: async ({ canvasElement }) => {
    const links = await exportWithoutSaveDialog(canvasElement, async () => {
      await expectToast(canvasElement, /Try again in about 15 seconds/, "error");
    });
    await expect(links).toHaveLength(0);
  },
};

/**
 * The gate can fill between the mint and the browser following the link. The
 * page cannot read the response the browser got, so it learns of the `429`
 * through the ticket's status, and says to wait as it would for the mint's.
 */
export const AGateThatFillsBeforeTheDownloadSaysToWait: Story = {
  render: story([{ state: "refused", status: 429, retry_after: 15 }]),
  play: async ({ canvasElement }) => {
    const links = await exportWithoutSaveDialog(canvasElement, async () => {
      await expectToast(canvasElement, /Try again in about 15 seconds/, "error");
    });
    await expect(links).toEqual([STORY_DOWNLOAD_URL]);
    await expectToast(canvasElement, /Could not export request log export/, "error");
  },
};

/**
 * A body cut short at the row or time limit ends the browser's download in a
 * failure. The page cannot take the partial file away, so the message says the
 * download is incomplete and not to use it.
 */
export const ATruncatedDownloadSaysTheFileIsIncomplete: Story = {
  render: story([
    { state: "streaming", rows: 1500 },
    { state: "truncated", rows: 1500, reason: "row_limit" },
  ]),
  play: async ({ canvasElement }) => {
    await exportWithoutSaveDialog(canvasElement, async () => {
      await expectToast(canvasElement, /The browser's download of it is incomplete/, "error");
    });
    const canvas = within(canvasElement);
    await expect(await canvas.findByRole("button", { name: "Export CSV" })).toBeEnabled();
  },
};

/** A download the control plane refused for another reason names its status. */
export const ARefusedDownloadNamesItsStatus: Story = {
  render: story([{ state: "refused", status: 401 }]),
  play: async ({ canvasElement }) => {
    await exportWithoutSaveDialog(canvasElement, async () => {
      await expectToast(canvasElement, /refused the download \(HTTP 401\)/, "error");
    });
  },
};

/**
 * Stop watching ends the page's polling and nothing else: the page cannot reach
 * the browser's download, so it says the download carries on rather than
 * claiming a cancel.
 */
export const StopWatchingLeavesTheDownloadToTheBrowser: Story = {
  render: story([{ state: "streaming", rows: 1200 }]),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await exportWithoutSaveDialog(canvasElement, async () => {
      await canvas.findByText(/rows written/);
      await userEvent.click(
        canvas.getByRole("button", { name: /the browser's download carries on/ }),
      );
      await expectToast(canvasElement, /Stopped following the export/, "success");
    });
    const reads = calls.calls.filter((c) => c.url.includes("/exports/tickets/")).length;
    await new Promise((resolve) => setTimeout(resolve, 1500));
    // no more status reads once the page stopped following
    await expect(calls.calls.filter((c) => c.url.includes("/exports/tickets/"))).toHaveLength(
      reads,
    );
    await expect(await canvas.findByRole("button", { name: "Export CSV" })).toBeEnabled();
  },
};

/**
 * A download the browser stopped (`client_closed`) is most likely the person
 * cancelling it in the download list: a neutral line, not a failure.
 */
export const ADownloadStoppedInTheBrowserIsNotAFailure: Story = {
  render: story([{ state: "truncated", rows: 40, reason: "client_closed" }]),
  play: async ({ canvasElement }) => {
    await exportWithoutSaveDialog(canvasElement, async () => {
      await expectToast(canvasElement, /The download stopped in the browser/, "success");
    });
    const canvas = within(canvasElement);
    await expect(canvas.queryByText(/Could not export/)).toBeNull();
  },
};
