# User docs (Mintlify)

Source for the polished, user-facing documentation served at the Rolter
[Mintlify](https://mintlify.com) site. These are the **product docs** for people
running Rolter — distinct from the contributor/architecture docs under
[`../docs/`](../docs/), which build to an mdBook on GitHub Pages.

- `docs.json` — Mintlify site config (theme, nav, tabs). Root of the site is this
  directory, so page hrefs are relative to `user-docs/` (e.g. `introduction`,
  `concepts/load-balancing`).
- `*.mdx` — page content in Mintlify MDX (supports `<Card>`, `<Steps>`,
  `<ParamField>`, `<CodeGroup>`, and other Mintlify components).

## Local preview

```bash
npm i -g mint
cd user-docs
mint dev
```

## Deploy

Mintlify builds and serves this directory to its own domain. Connect the
[Mintlify GitHub app](https://mintlify.com/docs/settings/github) to
`ormeilu/rolter` with the content directory set to `user-docs/`; pushes to the
default branch then trigger a redeploy. The build is independent of the mdBook
GitHub Pages workflow.
