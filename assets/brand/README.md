# Genatrix brand

The mark: on the left, the dot that reads everything; on the right, three bars
of falling length, what it hands back in order of what matters. Untidy in,
ordered out.

## Colours

| Name | Hex | Use |
| --- | --- | --- |
| Ink (墨黑) | `#2B2B2B` | the bars, text |
| Light ink (淡墨黑) | `#B8B8B8` | the bars on dark |
| Paper (宣纸白) | `#F7F0E8` | background |
| Inkstone (砚石灰) | `#6E756E` | secondary text |
| Plum (梅红) | `#C83C3C` | the dot, and nothing else loud |

The page also uses `#FCF9F5` (surface), `#E1DBD5` (line), `#AF2F30` (plum
for text on paper) and `#151312` (dark background).

## Files

| File | Where it goes |
| --- | --- |
| `genatrix-mark.svg`, `genatrix-mark-dark.svg` | the mark alone, for documents and the README |
| `genatrix-icon.svg`, `genatrix-icon-dark.svg` | the app icon, a rounded paper tile |
| `genatrix-icon-1024.png`, `genatrix-icon-dark-1024.png` | the same, rendered; `packaging/package-macos.sh` builds `Genatrix.icns` from the light one |
| `genatrix-small.svg`, `genatrix-small-dark.svg` | the mark drawn heavier for 32px and below |
| `genatrix-favicon.svg` | the page's icon, copied to `web/public/icon.svg` |
| `genatrix-touch.svg`, `genatrix-touch-180.png`, `genatrix-touch-512.png` | home screen and PWA icons, copied to `web/public/icon-180.png` and `icon-512.png` |
| `favicon-16.png`, `favicon-32.png` | the favicon for places that do not take SVG |
| `genatrix-menubar.svg`, `menubar-template*.png` | the one-colour template; the menu bar shell draws the same shape in code |

PNGs are rendered from the SVGs with headless Chrome:

```sh
chrome --headless=new --screenshot=out.png --window-size=1024,1024 \
  --default-background-color=00000000 file.svg
```
