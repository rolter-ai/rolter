# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.0.2](https://github.com/ormeilu/rolter/compare/v0.0.1...v0.0.2) - 2026-07-02

### Added

- *(control)* split config vs DB models, LiteLLM-style ([#17](https://github.com/ormeilu/rolter/pull/17))
- *(control)* serve versioned config snapshots for gateway polling ([#11](https://github.com/ormeilu/rolter/pull/11))
- *(store)* add postgres repository layer for tenancy/routing/limits ([#10](https://github.com/ormeilu/rolter/pull/10))
- *(store)* postgres-backed ConfigStore with sqlx migration runner [ROL-20 ROL-21] ([#9](https://github.com/ormeilu/rolter/pull/9))
- *(core)* scaffold rolter workspace and runnable gateway mvp

### Other

- release v0.0.1 ([#3](https://github.com/ormeilu/rolter/pull/3))

## [0.0.1](https://github.com/ormeilu/rolter/releases/tag/v0.0.1) - 2026-06-30

### Added

- *(core)* scaffold rolter workspace and runnable gateway mvp
