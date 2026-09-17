#!/usr/bin/env bun
// docs/user-docs/llms.txt, generated from the Mintlify nav (#1582).
//
// The file is an index for a coding agent: what Rolter is, the commands that
// matter, and every documentation page with the one line that says what is on
// it. Hand-maintaining that list is how it goes stale — a page is added to
// `docs.json` and the index keeps pointing at the eight pages that existed when
// someone last remembered. So it is generated from the nav plus each page's own
// frontmatter, and `--check` fails the build when the committed file and the
// docs have drifted apart.
//
// Links are site-relative on purpose. The docs site has no canonical domain yet
// (#1360); absolute urls would have to be guessed now and rewritten later,
// while a relative one resolves against whatever host serves the file.
//
//   bun scripts/gen-llms-txt.ts          # write the file
//   bun scripts/gen-llms-txt.ts --check  # fail if it is out of date

import { readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";

const DOCS = "docs/user-docs";
const OUT = join(DOCS, "llms.txt");

interface Group {
  group: string;
  pages: (string | Group)[];
}
interface Tab {
  tab: string;
  groups?: Group[];
}

/** the header, which is the part a person writes */
const PREAMBLE = `# Rolter

> An OpenAI- and Anthropic-compatible AI gateway and load balancer. One endpoint
> in front of many model providers: drop-in \`/v1\` compatibility, virtual keys
> with budgets, routing and failover across providers, and per-request cost and
> latency tracking. Self-hosted — every url below is the reader's own
> deployment, never a service Rolter operates.

Endpoints a client talks to, on a deployment of your own:

- \`POST {gateway}/v1/chat/completions\`, \`/v1/messages\`, \`/v1/embeddings\`,
  \`/v1/models\` — the data plane, OpenAI and Anthropic dialects side by side.
  Authenticate with a virtual key: \`Authorization: Bearer <key>\`.
- \`{control}/api/v1/...\` — the control plane (providers, routes, keys, limits),
  behind an operator session. \`{control}/openapi.json\` is its full contract.
- Model \`fake-llm\` is built into the gateway and needs no provider and no
  credential. Use it to prove a path end to end before any key exists.

Commands that matter:

- \`rolter easy-up\` — the zero-credential local stack: control plane, gateway
  and dashboard, no config file to write first.
- \`rolter gateway --config rolter.toml\` / \`rolter control\` — the two planes,
  run separately, which is how a real deployment runs them.
- \`rolter config export --output rolter.toml\` — write the live configuration
  back out as a file \`rolter-seed --import\` accepts. No credential is ever
  emitted.
- \`rolter mfa reset --email <address> --reason <why>\` — host-side break-glass
  for a lost second factor.

Reading order for an agent standing Rolter up for the first time: the setup
prompts below, then the quickstart, then the page for whatever it is wiring.
`;

/** `title` / `description` from a page's frontmatter */
function frontmatter(page: string): { title: string; description: string } {
  const source = readFileSync(join(DOCS, `${page}.mdx`), "utf8");
  const end = source.indexOf("\n---", 3);
  const head = source.startsWith("---") && end > 0 ? source.slice(3, end) : "";
  const read = (key: string) =>
    new RegExp(`^${key}:\\s*"?(.*?)"?\\s*$`, "m").exec(head)?.[1] ?? "";
  // the sidebar title is the short one — "Virtual Keys" rather than "Virtual
  // Keys: Client Authentication and Access Control", which is written for search
  return { title: read("sidebarTitle") || read("title") || page, description: read("description") };
}

function render(): string {
  const config = JSON.parse(readFileSync(join(DOCS, "docs.json"), "utf8")) as {
    navigation: { tabs: Tab[] };
  };
  const out: string[] = [PREAMBLE];
  for (const tab of config.navigation.tabs) {
    out.push(`## ${tab.tab}\n`);
    for (const group of tab.groups ?? []) {
      out.push(`### ${group.group}\n`);
      for (const page of group.pages) {
        if (typeof page !== "string") continue;
        const { title, description } = frontmatter(page);
        out.push(description ? `- [${title}](/${page}): ${description}` : `- [${title}](/${page})`);
      }
      out.push("");
    }
  }
  return `${out.join("\n").trimEnd()}\n`;
}

const wanted = render();
if (process.argv.includes("--check")) {
  const found = readFileSync(OUT, "utf8");
  if (found !== wanted) {
    console.error(
      `${OUT} is out of date with docs/user-docs/docs.json.\nRegenerate it with: bun scripts/gen-llms-txt.ts`,
    );
    process.exit(1);
  }
  console.log(`${OUT} matches the docs nav`);
} else {
  writeFileSync(OUT, wanted);
  console.log(`wrote ${OUT}`);
}
