# Moby protocol source snapshot

Copyright (c) 2026 Moby contributors.

The snapshot uses commit `59717ba` (branch `claude/agent-local-time`, based on
`claude/usb-control-full`) of the upstream Rust package `0.2.0`. This is an
unreleased commit: 0.2.0 has no tag yet. Replace it with the tagged release
when 0.2.0 ships. The `usb` module and its dependencies (`cobs`, `crc32fast`,
`hmac` 0.13, `sha2` 0.11) come along because the crate has no feature to omit
them; the API does not use them. [Upstream repository](https://github.com/mobyuz/moby-protocol)

The snapshot contains the upstream `Cargo.toml`, `LICENSE`, and `src/` tree.
Refresh them together when adopting a tagged protocol release. The license is
GPL-3.0-only. See [LICENSE](LICENSE).
