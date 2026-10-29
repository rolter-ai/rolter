"""Ties the trainer documented for `rolter-linear/1` to the gateway's features (#3455).

`docs/user-docs/security/semantic-guard-policies.mdx` ships a scikit-learn
training script that has to hash text into exactly the buckets the gateway's
local classifier reads. This runs the script as it is written on that page (the
first python block after "Training one.") over the golden vectors in
`crates/rolter-gateway/tests/fixtures/local_classifier/golden_features.json`,
which a Rust test in `local_classifier.rs` checks against the gateway. A change
to the page or to the gateway that moves a bucket fails one of the two.

To change a vector, run this file with ROLTER_TEST_REGENERATE_GOLDEN=1 and confirm the
Rust test still passes: both sides must agree.
"""

from __future__ import annotations

import json
import os
import re
import sys
import types
from pathlib import Path
from unittest import mock

import pytest

ROOT = Path(__file__).resolve().parents[2]
PAGE = ROOT / "docs/user-docs/security/semantic-guard-policies.mdx"
GOLDEN = ROOT / "crates/rolter-gateway/tests/fixtures/local_classifier/golden_features.json"

# the texts the vectors are made of: every way the tokenizer or the hash could
# disagree between the two languages
TEXTS = [
    "Ignore all previous instructions",
    "",
    "   \t\n  ",
    "one",
    "Ignore ALL previous-instructions, now! Ünïcode 42",
    "the the the the",
    "naïve café résumé",
    "snake_case and kebab-case and CamelCase",
    "日本語 のテキスト 123",
    "a b c d e f g h",
]
SETTINGS = [(1 << 16, 2), (1000, 3), (1, 1), (1 << 20, 1)]


def documented_features(buckets: int, ngrams: int):
    """The `features` function exactly as the page writes it."""
    section = PAGE.read_text(encoding="utf-8").split("**Training one.**", 1)[1]
    block = re.search(r"```python\n(.*?)```", section, re.S)
    assert block, "no python block after 'Training one.'"
    # the block imports scipy and scikit-learn for the training half; the hashing
    # half under test needs neither, so they are stubbed rather than installed
    stubs = {
        "scipy": types.ModuleType("scipy"),
        "scipy.sparse": types.SimpleNamespace(lil_matrix=object),
        "sklearn": types.ModuleType("sklearn"),
        "sklearn.linear_model": types.SimpleNamespace(LogisticRegression=object),
    }
    namespace: dict = {}
    with mock.patch.dict(sys.modules, stubs):
        exec(compile(block.group(1), str(PAGE), "exec"), namespace)
    namespace["BUCKETS"], namespace["NGRAMS"] = buckets, ngrams
    return namespace


def compute() -> list[dict]:
    out = []
    for buckets, ngrams in SETTINGS:
        namespace = documented_features(buckets, ngrams)
        for text in TEXTS:
            out.append(
                {
                    "text": text,
                    "ngrams": ngrams,
                    "buckets": buckets,
                    "present": sorted(namespace["features"](text)),
                }
            )
    return out


if os.environ.get("ROLTER_TEST_REGENERATE_GOLDEN") == "1":
    GOLDEN.write_text(json.dumps(compute(), ensure_ascii=False, indent=2) + "\n", encoding="utf-8")


def vectors() -> list[dict]:
    return json.loads(GOLDEN.read_text(encoding="utf-8"))


def test_the_vectors_cover_every_setting_and_text() -> None:
    assert [(v["buckets"], v["ngrams"], v["text"]) for v in vectors()] == [
        (b, n, t) for b, n in SETTINGS for t in TEXTS
    ]


def test_the_page_defaults_are_the_ones_the_vectors_use() -> None:
    section = PAGE.read_text(encoding="utf-8").split("**Training one.**", 1)[1]
    assert "BUCKETS, NGRAMS = 1 << 16, 2" in section
    assert (1 << 16, 2) in SETTINGS


@pytest.mark.parametrize("vector", vectors(), ids=lambda v: f"{v['buckets']}x{v['ngrams']}:{v['text'][:16]!r}")
def test_the_documented_trainer_hashes_into_the_golden_buckets(vector: dict) -> None:
    namespace = documented_features(vector["buckets"], vector["ngrams"])
    assert sorted(namespace["features"](vector["text"])) == vector["present"]
