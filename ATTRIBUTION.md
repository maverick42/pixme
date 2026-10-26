# Attribution

Third-party assets and adapted code shipped with PixMe. Licenses are kept
next to the assets.

## Assets

| Path | Title | Author | Source | License |
| --- | --- | --- | --- | --- |
| `assets/fonts/Inter-*.ttf` | Inter (Regular, Medium, SemiBold) | Rasmus Andersson | https://rsms.me/inter/ | SIL OFL 1.1 (`assets/fonts/OFL-Inter.txt`) |
| `assets/icons/*.svg` | Lucide icon set (subset: monitor, square-dashed, app-window, square-dashed-mouse-pointer, crop, pin, clock, folder, settings, x, chevron-down, check, plus, scan, grip-vertical, copy, download, folder-open, save, zoom-in, zoom-out, pencil, scaling, clipboard-paste, printer, mouse-pointer, mouse, mouse-pointer-click, message-square, message-circle, type, info, arrow-up-right, arrow-big-up, eraser, square, circle, highlighter, paint-bucket, smile, hand, text-cursor, ruler, loader-circle, circle-help, circle-x, lightbulb, image, undo-2, redo-2, stamp, brush, layout-grid) — `square-fill`, `circle-fill` and `pixelate` are original Lucide-style glyphs drawn for PixMe | Lucide contributors | https://lucide.dev | ISC (`assets/icons/LICENSE-lucide.txt`) |

## Adapted code

The visual style implementation — theme token structure, palette values,
icon pipeline and widget patterns in `src/ui/` — is adapted from PhotoCraft
(https://github.com/storytold/photocraft), copyright the ArtCraft Team and
PhotoCraft contributors, dual-licensed **MIT OR Apache-2.0**. The design is
reimplemented for PixMe; no code is copied verbatim. Original licenses:

- https://github.com/storytold/photocraft/blob/main/LICENSE-MIT
- https://github.com/storytold/photocraft/blob/main/LICENSE-APACHE
