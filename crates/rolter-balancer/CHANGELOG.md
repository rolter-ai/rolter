# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]
## [0.0.10](https://github.com/rolter-ai/rolter/compare/rolter-balancer-v0.0.9...rolter-balancer-v0.0.10) - 2026-07-21

### Features
- *(balancer)* add complexity routing primitives by [@ormeilu](https://github.com/ormeilu)
- *(balancer)* add precise and LMCache-aware routing [ROL-54] ([#522](https://github.com/rolter-ai/rolter/pull/522)) by [@ormeilu](https://github.com/ormeilu)
## [0.0.9](https://github.com/rolter-ai/rolter/compare/rolter-balancer-v0.0.8...rolter-balancer-v0.0.9) - 2026-07-15

### Miscellaneous
- update Cargo.toml dependencies
## [0.0.8](https://github.com/ormeilu/rolter/compare/rolter-balancer-v0.0.7...rolter-balancer-v0.0.8) - 2026-07-13

### Testing
- *(balancer)* adopt criterion for hot-path benchmarks ([#144](https://github.com/ormeilu/rolter/pull/144)) by [@ormeilu](https://github.com/ormeilu)
## [0.0.6](https://github.com/ormeilu/rolter/compare/rolter-balancer-v0.0.5...rolter-balancer-v0.0.6) - 2026-07-12

### Dependencies
- *(deps)* bump rand from 0.8.6 to 0.10.2 ([#125](https://github.com/ormeilu/rolter/pull/125)) by [@dependabot[bot]](https://github.com/dependabot[bot])

### Features
- *(balancer)* fastest latency-aware routing strategy ([#130](https://github.com/ormeilu/rolter/pull/130)) by [@ormeilu](https://github.com/ormeilu)
- *(balancer)* cheapest cost-aware routing strategy ([#128](https://github.com/ormeilu/rolter/pull/128)) by [@ormeilu](https://github.com/ormeilu)

## [0.0.4](https://github.com/ormeilu/rolter/compare/rolter-balancer-v0.0.3...rolter-balancer-v0.0.4) - 2026-07-10

### Added

- *(balancer)* bound the cache-aware trie with LRU node eviction ([#63](https://github.com/ormeilu/rolter/pull/63))
- *(balancer)* session-affinity scorer for warm-cache reuse ([#62](https://github.com/ormeilu/rolter/pull/62))
- *(balancer)* wire the scorer pipeline in as a selectable `pipeline` strategy ([#59](https://github.com/ormeilu/rolter/pull/59))
- *(balancer)* composable filter → weighted-score → argmax scorer pipeline ([#58](https://github.com/ormeilu/rolter/pull/58))
- *(balancer)* weighted selection honoring Target.weight [ROL-51] ([#50](https://github.com/ormeilu/rolter/pull/50))

## [0.0.2](https://github.com/ormeilu/rolter/compare/v0.0.1...v0.0.2) - 2026-07-02

### Added

- *(core)* scaffold rolter workspace and runnable gateway mvp

### Other

- release v0.0.1 ([#3](https://github.com/ormeilu/rolter/pull/3))

## [0.0.1](https://github.com/ormeilu/rolter/releases/tag/v0.0.1) - 2026-06-30

### Added

- *(core)* scaffold rolter workspace and runnable gateway mvp
