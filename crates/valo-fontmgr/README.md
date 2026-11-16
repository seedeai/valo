# valo-fontmgr

A unified API for system font lookup: families, styles, character fallback, and the system UI font. Returns font bytes and collection indices.

## Apple (macOS, iOS, iPadOS)

Uses CoreText.

## Android

Current: reads `/system/etc/fonts.xml` and `/system/fonts`. Partial family and fallback support.

Planned API: `AFontMatcher`, `AFont`, `ASystemFontIterator`. Requires Android 10 / API 29+.

TODOs:

- Replace XML matching with native APIs.
- Define support for named families and aliases.
- Preserve variable-font coordinates.
- Test native matching on Android.

## Windows

Backend: `fontdb` directory scan. No DirectWrite integration.

## Linux and other native targets

Backend: `fontdb` directory scan.

Scanning covers families, styles, and glyph coverage; no language-aware fallback, UI font lookup, or change notifications.

## WebAssembly

API types only. Fonts are supplied by the host.
