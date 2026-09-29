import type { Meta, StoryObj } from "@storybook/react-vite";
import { useTranslation } from "react-i18next";
import { expect, within } from "storybook/test";

import en from "@/lib/i18n/locales/en.json";
import ru from "@/lib/i18n/locales/ru.json";
import { useFormat } from "@/lib/i18n/format";
import { expectDrawnIn, tokenFamily } from "@/lib/story-fonts";

// The preview used to import `index.css` without the fontsource packages
// `main.tsx` loads, so `--font-sans` and `--font-mono` fell through to the
// browser's fallbacks in every story: a Courier-like mono, and in `ru` a serif
// for the "мс" unit, since that fallback has no Cyrillic (#2051). Both entries
// now import `lib/fonts.ts`. These stories check the result on screen, with
// real catalog copy: a sentence in the body face and a latency in the mono
// face, which is where the serif showed up.

function Specimen() {
  const { t } = useTranslation();
  const format = useFormat();
  return (
    <div className="grid max-w-xl gap-3">
      <p data-testid="sans" className="text-sm">
        {t("analytics.noRowsYet")}
      </p>
      <p data-testid="mono" className="font-mono text-xs">
        {t("analytics.ms", { value: format.number(412) })}
      </p>
    </div>
  );
}

const meta = {
  title: "Behaviour/Fonts",
  component: Specimen,
} satisfies Meta<typeof Specimen>;

export default meta;
type Story = StoryObj<typeof meta>;

/** Latin copy is drawn in Geist and Geist Mono, not in the browser's fallbacks. */
export const DrawsLatinInGeist: Story = {
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const mono = await canvas.findByText(en.analytics.ms.replace("{{value}}", "412"));
    await expect(tokenFamily("--font-mono")).toBe("Geist Mono Variable");
    await expect(tokenFamily("--font-sans")).toBe("Geist Variable");
    await expectDrawnIn(mono, "--font-mono");
    await expectDrawnIn(canvas.getByTestId("sans"), "--font-sans");
  },
};

/**
 * Russian copy too: the vendored subsets carry Cyrillic, so the "мс" unit
 * stays in Geist Mono instead of dropping to a serif mid-cell.
 */
export const DrawsCyrillicInGeist: Story = {
  globals: { locale: "ru" },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const mono = await canvas.findByText(ru.analytics.ms.replace("{{value}}", "412"));
    await expect(tokenFamily("--font-mono")).toBe("Geist Mono Variable");
    await expectDrawnIn(mono, "--font-mono");
    await expectDrawnIn(canvas.getByTestId("sans"), "--font-sans");
  },
};
