// Colour-token fixture for stories that assert a computed colour.
//
// A story compares an element's computed `color` with what the token resolves
// to on the same page, rather than with a literal `rgb()`, so the assertion
// follows the token when the design retunes it and still fails when a
// component drifts to a raw palette colour or to the wrong half of a status
// pair (docs/dev-docs/development/dashboard-theme.md).
//
// Not a `.stories.tsx` file: it is a fixture, like `story-viewport.ts`.

/** The computed `color` that `var(<token>)` resolves to on this page. */
export function resolveColorToken(token: `--${string}`): string {
  const probe = document.createElement("span");
  probe.style.color = `var(${token})`;
  document.body.appendChild(probe);
  const color = getComputedStyle(probe).color;
  probe.remove();
  return color;
}
