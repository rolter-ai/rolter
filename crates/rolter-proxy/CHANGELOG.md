# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]
## [0.0.6](https://github.com/ormeilu/rolter/compare/rolter-proxy-v0.0.5...rolter-proxy-v0.0.6) - 2026-07-12

### Features
- *(gateway)* add /v1/audio/transcriptions + /v1/audio/translations ([#110](https://github.com/ormeilu/rolter/pull/110)) by [@ormeilu](https://github.com/ormeilu)

## [0.0.5](https://github.com/ormeilu/rolter/compare/rolter-proxy-v0.0.4...rolter-proxy-v0.0.5) - 2026-07-11

### Added

- *(gateway)* propagate caller trace context to upstream ([#96](https://github.com/ormeilu/rolter/pull/96))
- *(gateway)* provider status-page secondary health signal [ROL-200] ([#90](https://github.com/ormeilu/rolter/pull/90))
- *(gateway)* opt-in also_track_via_llm_call end-to-end health check ([#89](https://github.com/ormeilu/rolter/pull/89))
- *(core)* multiple weighted api keys per provider ([#83](https://github.com/ormeilu/rolter/pull/83))

### Other

- *(proxy)* golden wire tests proving no rolter signature upstream ([#81](https://github.com/ormeilu/rolter/pull/81))

## [0.0.4](https://github.com/ormeilu/rolter/compare/rolter-proxy-v0.0.3...rolter-proxy-v0.0.4) - 2026-07-10

### Added

- *(gateway)* upstream timeouts + graceful shutdown/drain [ROL-52] ([#48](https://github.com/ormeilu/rolter/pull/48))

## [0.0.2](https://github.com/ormeilu/rolter/compare/v0.0.1...v0.0.2) - 2026-07-02

### Added

- *(core)* scaffold rolter workspace and runnable gateway mvp

### Other

- release v0.0.1 ([#3](https://github.com/ormeilu/rolter/pull/3))

## [0.0.1](https://github.com/ormeilu/rolter/releases/tag/v0.0.1) - 2026-06-30

### Added

- *(core)* scaffold rolter workspace and runnable gateway mvp
