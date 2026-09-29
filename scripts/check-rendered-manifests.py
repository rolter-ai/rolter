#!/usr/bin/env python3
# /// script
# requires-python = ">=3.9"
# dependencies = ["pyyaml==6.0.3"]
# ///
"""Fail on a rendered chart that is not strictly valid YAML.

`helm template` output is loaded by whatever is downstream — kubectl with
server-side apply, Argo CD, Flux, a policy engine — and those are stricter than
the loader helm itself uses. A duplicate mapping key is the case that bit us
(#1090): it renders, it applies, and the day a template reorders the two lines
every environment variable silently disappears. PyYAML is lenient about it too,
so the duplicate check is explicit.

Reads a multi-document manifest on stdin.

    helm template ci charts/rolter | uv run --script scripts/check-rendered-manifests.py

`--script` installs the pyyaml pinned above and nothing else. The repo root's
pyproject.toml is the rolter maturin wheel, and a `uv run` that treats it as
the project builds that wheel before the script starts, minutes of work the
check never uses (#1901).
"""

import sys

import yaml


class StrictLoader(yaml.SafeLoader):
    """SafeLoader that refuses a mapping with the same key twice."""


def no_duplicate_keys(loader, node, deep=False):
    mapping = {}
    for key_node, value_node in node.value:
        key = loader.construct_object(key_node, deep=deep)
        if key in mapping:
            mark = key_node.start_mark
            raise yaml.constructor.ConstructorError(
                None,
                None,
                f"duplicate key {key!r} in mapping",
                mark,
            )
        mapping[key] = loader.construct_object(value_node, deep=deep)
    return mapping


StrictLoader.add_constructor(
    yaml.resolver.BaseResolver.DEFAULT_MAPPING_TAG, no_duplicate_keys
)


def main() -> int:
    text = sys.stdin.read()
    try:
        documents = [doc for doc in yaml.load_all(text, StrictLoader) if doc]
    except yaml.YAMLError as err:
        print(f"::error::rendered manifest is not strictly valid YAML: {err}")
        return 1
    if not documents:
        print("::error::rendered manifest is empty")
        return 1
    print(f"{len(documents)} document(s) parsed with a strict loader")
    return 0


if __name__ == "__main__":
    sys.exit(main())
