// the tokeniser chunk (#949).
//
// This module is the only place refractor is imported, and `CodeBlock` reaches
// it through a dynamic `import()`. That keeps the grammars in a chunk of their
// own: a screen that shows no code never downloads them, and the code block
// renders its plain text on the first frame either way.
//
// refractor is Prism's grammars packaged as ESM with a hast return value —
// vendored through `bun add`, tokenised in the browser, with no network of any
// kind at runtime. See docs/development/highlighting.md for why Prism and not
// Shiki, and for how to add a language.
import type { Element, RootContent } from "hast";
import bash from "refractor/bash";
import { refractor } from "refractor/core";
import csv from "refractor/csv";
import javascript from "refractor/javascript";
import json from "refractor/json";
import log from "refractor/log";
import markdown from "refractor/markdown";
import python from "refractor/python";
import sql from "refractor/sql";
import toml from "refractor/toml";
import typescript from "refractor/typescript";
import yaml from "refractor/yaml";

import {
  HIGHLIGHT_CHAR_LIMIT,
  type CodeLanguage,
  type CodeNode,
} from "@/lib/code";

// one entry per non-`text` member of CODE_LANGUAGES. refractor pulls each
// grammar's own dependencies in for us (typescript extends javascript, which
// extends clike), so this list is the languages, not the graph
const GRAMMARS = [
  bash,
  csv,
  javascript,
  json,
  log,
  markdown,
  python,
  sql,
  toml,
  typescript,
  yaml,
];

let registered = false;

function register(): void {
  if (registered) return;
  for (const grammar of GRAMMARS) refractor.register(grammar);
  registered = true;
}

/**
 * Tokenise `value` as `language`.
 *
 * Never throws: a grammar that is not registered, plain text, and anything past
 * the size cap all fall back to the source as a single text node, so the caller
 * renders the same content either way and only loses the colour.
 */
export function highlight(value: string, language: CodeLanguage): CodeNode[] {
  if (language === "text" || value.length > HIGHLIGHT_CHAR_LIMIT) return [value];
  register();
  if (!refractor.registered(language)) return [value];
  try {
    return convert(refractor.highlight(value, language).children);
  } catch {
    // a grammar throwing on pathological input is a display bug, not a reason
    // to lose the payload an operator opened the drawer to read
    return [value];
  }
}

function convert(children: readonly RootContent[]): CodeNode[] {
  const out: CodeNode[] = [];
  for (const child of children) {
    if (child.type === "text") out.push(child.value);
    else if (child.type === "element") out.push(element(child));
  }
  return out;
}

function element(node: Element): CodeNode {
  const className = node.properties.className;
  return {
    className: Array.isArray(className) ? className.join(" ") : String(className ?? ""),
    children: convert(node.children),
  };
}
