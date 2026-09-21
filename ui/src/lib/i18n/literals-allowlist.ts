// the strings `check:literals` reports that are not copy (#958).
//
// this replaced `literals-baseline.json`, a recorded list of every literal the
// gate tolerated. that list only ever meant "not translated yet"; this one means
// "never translated", so every entry carries the reason a translator would give
// for leaving it alone. add one by hand, in review, only for notation — a label,
// a sentence or an error an operator can read belongs in the catalogs. see
// docs/dev-docs/development/i18n.md
import type { AllowList } from "./literals";

export const NOT_COPY: AllowList = {
  "src/components/CodeSnippetDialog.tsx": {
    Python: "the language's own name, spelled the same in every locale",
    JavaScript: "the language's own name, spelled the same in every locale",
  },
  "src/components/ui/code-block.tsx": {
    Bash: "the shell's own name, spelled the same in every locale",
    Python: "the language's own name, spelled the same in every locale",
    JavaScript: "the language's own name, spelled the same in every locale",
    TypeScript: "the language's own name, spelled the same in every locale",
    Markdown: "the format's own name, spelled the same in every locale",
  },
  "src/components/ParamsEditor.tsx": {
    temperature:
      "an example parameter name in the name field's placeholder; it is the key the upstream API reads, typed verbatim",
  },
  "src/components/ProviderGroupSheet.tsx": {
    "{…}/model · {…}":
      "the group's address pattern (`slug/model`) and its strategy id, both typed verbatim into client config",
  },
  "src/lib/auth.tsx": {
    "useAuth must be used within AuthProvider":
      "a developer invariant thrown when a component renders outside the provider; no operator path reaches it",
  },
  "src/pages/AcceptInvite.tsx": {
    rolter: "the product's name in the wordmark, spelled the same in every locale",
  },
  "src/pages/Connectors.tsx": {
    datadog:
      "an example connector name in the name field's placeholder: the vendor's own name, spelled the same in every locale",
  },
  "src/pages/Limits.tsx": {
    "{…} rpm":
      "a rate unit named after the `rpm` field the limit is set through; the badge is notation",
    "{…} tpm":
      "a rate unit named after the `tpm` field the limit is set through; the badge is notation",
  },
  "src/pages/Login.tsx": {
    rolter: "the product's name in the wordmark, spelled the same in every locale",
  },
  "src/pages/Playground.tsx": {
    nova: "a voice id the speech endpoint takes verbatim (OpenAI's own names), spliced into the translated `voiceOption` label",
    onyx: "a voice id the speech endpoint takes verbatim (OpenAI's own names), spliced into the translated `voiceOption` label",
    shimmer:
      "a voice id the speech endpoint takes verbatim (OpenAI's own names), spliced into the translated `voiceOption` label",
  },
  "src/pages/PromptRepository.tsx": {
    "v{…}": "a version number in `v3` notation, the same in every locale",
  },
  "src/pages/SkillsRepository.tsx": {
    "v{…}": "a version number in `v3` notation, the same in every locale",
  },
};
