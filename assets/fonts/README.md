# Test fonts

Third-party fonts used by valo's golden tests and examples. **No valo crate embeds or ships a font** — these are read from disk by tests and examples only. Full copyright notices and the license text are in [OFL.txt](OFL.txt).

| file | font | why it's here |
|---|---|---|
| `fira_sans.ttf` | Fira Sans Regular (Mozilla) | the default latin face for goldens and examples |
| `jetbrains_mono.ttf` | JetBrains Mono | the monospace face the HUD draws with |
| `noto_sans_arabic.ttf` | Noto Sans Arabic | bidi and complex shaping coverage |
| `noto_sans_hebrew.ttf` | Noto Sans Hebrew | right-to-left coverage |
| `noto_color_emoji_subset.ttf` | Noto Color Emoji, subset to 6 emoji | the CBDT color-bitmap path |
| `noto_color_emoji_colrv1_subset.ttf` | Noto Color Emoji, COLRv1 subset | the skrifa COLRv1 paint-graph path |
| `fa_regular_400.woff2` | Font Awesome Free | the WOFF2 decompression path, and icon-font fallback behavior |

All font files are licensed under the SIL Open Font License 1.1. Font Awesome's icons are additionally CC BY 4.0 and its code MIT; only the font file is vendored here.

The emoji subsets were produced with fontTools: `noto_color_emoji_subset.ttf` covers U+1F600, U+1F680, U+1F3A8, U+2728, U+1F49B, and U+1F308.
