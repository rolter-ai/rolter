// Reading the shipped stylesheet from a story, for the rules a story run cannot
// switch on. The story browser is a fine pointer with colours unforced, and
// neither `@media (pointer: coarse)` nor `@media (forced-colors: active)` can be
// turned on from inside a page, so a story asserts the rule the build emitted
// instead: it exists, it sits under the right condition, and what it declares
// resolves to the value the design asks for.

/** the declarations of the first rule matching `selector` inside a `@media` whose condition matches `condition` */
export function mediaDeclarations(
  condition: RegExp,
  selector: string | RegExp,
): CSSStyleDeclaration | null {
  const matches = (text: string) =>
    typeof selector === "string" ? text === selector : selector.test(text);
  const walk = (rules: CSSRuleList, inside: boolean): CSSStyleDeclaration | null => {
    for (const rule of Array.from(rules)) {
      if (rule instanceof CSSStyleRule) {
        if (inside && matches(rule.selectorText)) return rule.style;
        const nested = rule.cssRules?.length ? walk(rule.cssRules, inside) : null;
        if (nested) return nested;
      } else if (rule instanceof CSSMediaRule) {
        const here = inside || condition.test(rule.conditionText || rule.media.mediaText);
        const found = walk(rule.cssRules, here);
        if (found) return found;
      } else if ("cssRules" in rule && (rule as CSSGroupingRule).cssRules) {
        // @layer, @supports: transparent to the media condition
        const found = walk((rule as CSSGroupingRule).cssRules, inside);
        if (found) return found;
      }
    }
    return null;
  };
  for (const sheet of Array.from(document.styleSheets)) {
    let rules: CSSRuleList;
    try {
      rules = sheet.cssRules;
    } catch {
      continue;
    }
    const found = walk(rules, false);
    if (found) return found;
  }
  return null;
}

/** what a length declaration resolves to in pixels, by drawing it on a probe */
export function resolvePx(length: string): number {
  const probe = document.createElement("div");
  probe.style.cssText = `position:absolute;visibility:hidden;height:${length}`;
  document.body.append(probe);
  const px = probe.getBoundingClientRect().height;
  probe.remove();
  return px;
}
