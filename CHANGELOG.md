# Changelog

## 0.1.0

- Borrowed RFC 5322 header framing, unfolding and structured MIME fields.
- Iterative multipart and message/rfc822 tree parsing with caller-owned storage.
- Lazy transfer decoding and explicit UTF-8/ASCII/Latin-1 text conversion.
- Resource limits, strict errors, portable scans and platform-specific acceleration.
- RFC/property/adversarial tests, pinned mail-parser comparison, Miri and cross-platform CI.
- Default-branch release workflow with version bumping and idempotent recovery.
- Complete-input diagnostic offsets and explicit rejection of ambiguous boundaries
  and unsupported extended charset parameters.
- Executable inspection example, portable unsafe-code prohibition, and native
  architecture benchmark workflow with full metadata consumption.
