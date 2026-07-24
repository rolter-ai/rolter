# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]
## [0.0.11](https://github.com/rolter-ai/rolter/compare/rolter-proxy-v0.0.10...rolter-proxy-v0.0.11) - 2026-07-24

### Features
- *(core)* expand provider adapter kind coverage [ROL-132] ([#645](https://github.com/rolter-ai/rolter/pull/645)) by [@ormeilu](https://github.com/ormeilu)
## [0.0.10](https://github.com/rolter-ai/rolter/compare/rolter-proxy-v0.0.9...rolter-proxy-v0.0.10) - 2026-07-21

### Features
- *(proxy)* add xai (grok) hosted provider kind ([#600](https://github.com/rolter-ai/rolter/pull/600)) by [@ormeilu](https://github.com/ormeilu)
- *(proxy)* add gemini/mistral/groq + native gemini generateContent kinds ([#598](https://github.com/rolter-ai/rolter/pull/598)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* implement medium-priority platform enhancements [ROL-65] ([#525](https://github.com/rolter-ai/rolter/pull/525)) by [@ormeilu](https://github.com/ormeilu)
- *(balancer)* add precise and LMCache-aware routing [ROL-54] ([#522](https://github.com/rolter-ai/rolter/pull/522)) by [@ormeilu](https://github.com/ormeilu)
- *(proxy)* add rotating egress proxy pools [ROL-101] ([#520](https://github.com/rolter-ai/rolter/pull/520)) by [@ormeilu](https://github.com/ormeilu)

### Performance
- *(proxy)* eliminate unnecessary clones and allocations in translation ([#597](https://github.com/rolter-ai/rolter/pull/597)) by [@ormeilu](https://github.com/ormeilu)
## [0.0.9](https://github.com/rolter-ai/rolter/compare/rolter-proxy-v0.0.8...rolter-proxy-v0.0.9) - 2026-07-15

### Features
- *(store)* add immutable URL-safe provider slug for model addressing ([#191](https://github.com/rolter-ai/rolter/pull/191)) by [@ormeilu](https://github.com/ormeilu)

### Testing
- *(proxy)* regenerate expired TLS fixture certificates ([#203](https://github.com/rolter-ai/rolter/pull/203)) by [@ormeilu](https://github.com/ormeilu)
## [0.0.8](https://github.com/ormeilu/rolter/compare/rolter-proxy-v0.0.7...rolter-proxy-v0.0.8) - 2026-07-13

### Bug Fixes
- *(gateway)* reject unsupported response resources ([#163](https://github.com/ormeilu/rolter/pull/163)) by [@ormeilu](https://github.com/ormeilu)

### Features
- *(proxy)* support custom ca bundles ([#168](https://github.com/ormeilu/rolter/pull/168)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* support responses lifecycle resources ([#166](https://github.com/ormeilu/rolter/pull/166)) by [@ormeilu](https://github.com/ormeilu)
- *(proxy)* normalize provider role capabilities [ROL-262] ([#164](https://github.com/ormeilu/rolter/pull/164)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* add responses api translation ([#162](https://github.com/ormeilu/rolter/pull/162)) by [@ormeilu](https://github.com/ormeilu)
- *(proxy)* translate openai and anthropic APIs ([#159](https://github.com/ormeilu/rolter/pull/159)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* add cloud provider health adapters ([#157](https://github.com/ormeilu/rolter/pull/157)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* add OpenRouter provider ([#153](https://github.com/ormeilu/rolter/pull/153)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* add self-hosted ollama provider ([#150](https://github.com/ormeilu/rolter/pull/150)) by [@ormeilu](https://github.com/ormeilu)
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
