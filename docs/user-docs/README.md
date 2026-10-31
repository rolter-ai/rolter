# User docs (Docs7)

Source for the polished, user-facing documentation, published by
[Docs7](https://context7.com/docs/docs7/overview) at
<https://rolter-ai-rolter.docs7.io/>. These are the **product docs** for people
running Rolter — distinct from the contributor/architecture docs under
[`../dev-docs/`](../dev-docs/), which build to an mdBook on GitHub Pages at
<https://rolter-ai.github.io/rolter/>.

- `docs.json` — site config (theme, nav, tabs) in the `docs.json` format that
  Mintlify established and Docs7 reads; `$schema` points at Docs7's schema. Root
  of the site is this directory, so page hrefs are relative to `docs/user-docs/`
  (e.g. `introduction`, `concepts/load-balancing`).
- `*.mdx` — page content in MDX, using the components that format defines
  (`<Card>`, `<Steps>`, `<ParamField>`, `<CodeGroup>` and the rest).
- `llms.txt` — the agent index, generated from the `docs.json` nav by
  `bun scripts/gen-llms-txt.ts` and served at the site root.

## Local preview

Needs Node.js 20.19 or newer.

```bash
cd docs/user-docs
npx @upstash/docs7 dev
```

## Deploy

The Context7 GitHub app builds this directory. A pull request that changes it
gets a `Docs7 Preview` check, and a push to `master` redeploys the site. The
build is independent of the mdBook GitHub Pages workflow.

Social previews need no domain in `docs.json`: `seo.metatags` keeps the
root-relative `/images/og.png`, and Docs7 renders `og:image`, `twitter:image` and
the canonical link as absolute URLs against the site's own domain (#1360). Keep
it relative, so a domain change needs no edit here.
