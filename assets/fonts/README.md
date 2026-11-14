# Test fonts

Third-party fonts used by valo's golden tests and examples. **No valo crate embeds or ships a font** — these are read from disk by tests and examples only. Full copyright notices and the license text are in [OFL.txt](OFL.txt).

| file | font | why it's here |
|---|---|---|
| `fira_sans.ttf` | Fira Sans Regular (Mozilla) | the default latin face for goldens and examples |
| `line_gap_test.ttf` | Fira Sans Regular, Latin letters only, with a line gap, renamed "Line Gap Test" | how a font's line gap is shared above and below its lines |
| `newline_test.ttf` | Noto Sans Hebrew, Hebrew letters and the space only, with U+000A mapped to the space glyph, renamed "Newline Test" | a fallback face whose cmap maps the line feed, as some system fonts' do, which a hard break must not resolve to |
| `jetbrains_mono.ttf` | JetBrains Mono | the monospace face the HUD draws with |
| `noto_sans_arabic.ttf` | Noto Sans Arabic | bidi and complex shaping coverage |
| `noto_sans_hebrew.ttf` | Noto Sans Hebrew | right-to-left coverage |
| `noto_color_emoji_subset.ttf` | Noto Color Emoji, subset to 6 emoji | the CBDT color-bitmap path |
| `noto_color_emoji_colrv1_subset.ttf` | Noto Color Emoji, COLRv1 subset | the skrifa COLRv1 paint-graph path |
| `fa_regular_400.woff2` | Font Awesome Free | the WOFF2 decompression path, and icon-font fallback behavior |
| `noto_sans_sc_ni.woff2` | Noto Sans SC, Google Fonts `text=你` subset | WOFF2 transformed glyf with `overlapSimpleBitmap` |

All font files are licensed under the SIL Open Font License 1.1. Font Awesome's icons are additionally CC BY 4.0 and its code MIT; only the font file is vendored here.

`line_gap_test.ttf` was made from `fira_sans.ttf` with fontTools: subset to A–Z, a–z and the space, its `OS/2` typographic and `hhea` line gaps set to 200 of 1000 units, and its family renamed, as the licence asks of a modified font.

`newline_test.ttf` was made from `noto_sans_hebrew.ttf` with fontTools: subset to U+05D0–U+05EA and the space, U+000A added to its Unicode cmaps as the space's glyph, and its family renamed.

The emoji subsets were produced with fontTools: `noto_color_emoji_subset.ttf` covers U+1F600, U+1F680, U+1F3A8, U+2728, U+1F49B, and U+1F308.
