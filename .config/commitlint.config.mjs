// conventional commits config for commit messages and pr titles
// used by ci (pr title check) and the conventional-pre-commit hook
export default {
  extends: ["@commitlint/config-conventional"],
  rules: {
    "type-enum": [
      2,
      "always",
      [
        "feat",
        "fix",
        "perf",
        "refactor",
        "docs",
        "test",
        "build",
        "ci",
        "chore",
        "revert",
      ],
    ],
    "scope-enum": [
      1,
      "always",
      [
        "gateway",
        "balancer",
        "proxy",
        "core",
        "store",
        "auth",
        "control",
        "ui",
        "docs",
        "infra",
        "ci",
        "deps",
        "release",
      ],
    ],
    "subject-case": [2, "always", "lower-case"],
    "header-max-length": [2, "always", 72],
  },
};
