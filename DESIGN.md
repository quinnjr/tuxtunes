# Design

> Maintained with frontend-god-mode.
> Source of truth for typography, color, motion, layout, and component tokens.
> Read this BEFORE touching the UI in any subsequent session.

## Aesthetic direction

macOS-native desktop utility — a Tauri music library/player that clones
Apple Music's AppKit chrome (graphite neutrals, one saturated red, SF-like
type, hairline separators, vibrancy blur) rather than looking like a web
app. "A real app that happens to be built with web tech."

## Dials

- DESIGN_VARIANCE: 2 / 10 (AppKit is symmetric and conventional on purpose)
- MOTION_INTENSITY: 2 / 10 (hover fills, a 2px card lift — no physics)
- VISUAL_DENSITY: 8 / 10 (track lists, device detail, settings are dense)

Deliberate deviation from the global defaults: this project targets a
specific platform idiom, so low variance and low motion are the aesthetic,
not an accident. Do not "add energy" here.

## Type stack

- Display + body: Inter (SF's freely-licensed stand-in), weights 400/500/600/700
- Mono: none — tabular figures come from Inter via `.tnum`
  (`font-variant-numeric: tabular-nums`)
- Loaded via: `@fontsource/inter` (bundled locally; the Tauri webview must
  never depend on network fonts)
- Scale (HIG-named, `@theme`): `--text-caption2` 10px · `--text-caption`
  11px · `--text-footnote` 12px · `--text-body` 13px (the base; macOS
  control size) · `--text-headline` 15px

Banned in this project: swapping Inter for a "designer" face (Geist,
Satoshi, serif) — it breaks the AppKit illusion. No serif anywhere.

## Color tokens (hex; macOS graphite)

Dark is the default; `:root[data-theme='light']` swaps the palette.

```css
@theme {
  --color-bg-primary: #1e1e1e; /* content */
  --color-bg-surface: #28282a; /* panels, cards */
  --color-bg-transport: #262628; /* toolbar base, seen through blur */
  --color-bg-elevated: #323234; /* inset / filled control */
  --color-bg-tertiary: #3a3a3c; /* art placeholders, chips */
  --color-accent: #fa233b; /* Apple Music red: fills, rings, borders */
  --color-accent-hover: #fb4d5c;
  --color-accent-text: #ff6b77; /* red as text/icon on dark: 6.05:1 */
  --color-accent-strong: #e01f37; /* red fill under white text: 4.75:1 */
  --color-accent-strong-hover: #c4121f; /* darker; white stays 6.08:1 */
  --color-text-primary: #f5f5f7;
  --color-text-secondary: #a1a1a6;
  --color-text-muted: #9a9a9f; /* 5.95:1 on content, 4.57:1 on elevated */
  --color-border: #38383a; /* hairline */
  --color-border-strong: #48484a;
}
```

Light overrides that matter: `--color-text-muted: #6e6e73` (5.07:1 on
white), `--color-accent-text` / `--color-accent-strong: #c4121f` (6.08:1).

Red comes in **three cuts** for contrast, not for decoration:

| Utility                                             | Use                                                                 | Why                                                          |
| --------------------------------------------------- | ------------------------------------------------------------------- | ------------------------------------------------------------ |
| `bg-accent` / `ring-accent` / `border-accent`       | fills, focus rings, progress bars                                   | brand red, nothing sits on it                                |
| `text-accent-text`                                  | red _as_ text or an icon (current track, destructive items, errors) | brand red is 4.26:1 on dark, 3.91:1 on white — under AA      |
| `bg-accent-strong` + `hover:bg-accent-strong-hover` | a red fill carrying **white** text (selected row, accent button)    | white on brand red is 3.91:1; on `accent-hover` it is 3.32:1 |

Banned in this project: `text-accent` on small text (use
`text-accent-text`); white on `bg-accent` (use `bg-accent-strong`);
`hover:bg-accent-hover` on any fill carrying white text;
pure `#000`/`#fff` on a surface; any second accent hue; gradients beyond
the faint toolbar top-light.

## Shadows

Neutrals are tinted to the background, never pure black:

```css
--shadow-card-hover: 0 10px 20px color-mix(in srgb, #000000 35%, transparent);
--shadow-sheet: 0 25px 50px -12px color-mix(in srgb, #000000 60%, transparent);
```

The album card's edge is an **inset ring** (`inset 0 0 0 1px …12%`), not a
drop shadow — it keeps light artwork from bleeding without outlining dark
artwork. The Now Playing widget is the one recessed surface (inset bezel).

## Motion

No springs, no bounce — AppKit does not bounce. This is the deliberate
inverse of the global motion default.

- Transitions: color/background `80–160ms ease`; card lift `160ms ease`
- Card hover: `translateY(-2px)` + shadow only — **no scale** (scale reads as web)
- Animate **transform and opacity only** — progress bars use `scaleX`,
  never `width`
- `@media (prefers-reduced-motion: reduce)` collapses every transition and
  animation to `0.01ms` (global block in `styles.css`)
- Banned: spring/elastic easing, `transition-all`, animating `width`/`height`

## Layout

- Shell: frameless Tauri window, `h-[100dvh]` column —
  menu bar → transport bar → (sidebar + content) → status bar
- Sidebar: fixed `w-64`, vibrancy blur; content fills the remainder
  (`flex-1 min-w-0`)
- Two modal geometries only: small centered sheet
  (`mac-sheet`, 360–680px wide) and the full-height Settings sheet
  (`max-w-[760px]`)
- Overlays are opaque `mac-sheet`, **not** vibrancy — blur behind text
  fields hurts legibility
- No marketing layout patterns (no hero, no cards-in-a-row) — this is an app

## Component inventory

All custom; no shadcn/component library. Canonical classes in `styles.css`:

- `mac-toolbar` — translucent top-light chrome (menu bar, transport, headers)
- `mac-vibrancy` — heavier blur for tall surfaces (sidebar)
- `mac-sheet` / `mac-sheet-backdrop` — modal surface + dimmer
- `mac-btn` | `mac-sidebar-item` | `mac-segment` | `mac-segmented`
- `mac-card` (album art) | `mac-pill-lcd` (Now Playing readout)
- `hairline-b` / `hairline-t` / `hairline-r` | `.tnum`
- `win-control` — frameless caption buttons (Linux/Windows)
- `appModalSheet` — dialog semantics + focus trap for every sheet
- `appRovingFocus` — the shared keyboard model for `radiogroup`/`listbox`/`menu`
- Icons: FontAwesome (`@fortawesome/*`) for semantic glyphs;
  typographic marks (`♫ ✓ ▲▼ → × ⤢ ⚙`) stay as type, not icons

## Project-specific bans

- No `text-accent` on small text; no white on `bg-accent` (see the three-cut table)
- No second accent hue — red means selection / playing / primary / error
- No `h-screen` (use `h-[100dvh]`)
- No emoji-presentation glyphs as icons (FontAwesome instead)
- No scale-on-hover (translate + shadow only)
- Placeholder-only labels are banned — every input has a `<label>` or `aria-label`

## Accessibility floor

- WCAG 2.2 AA on all body copy (≥ 4.5:1) — verified for every palette pair,
  including hover fills (`accent-strong-hover`, not `accent-hover`)
- Every modal: `role="dialog"` + `aria-modal` + focus trap + Escape
  (via `appModalSheet`; the app shell is set `[inert]` while any modal is
  open, so SR/Tab cannot reach the UI behind it). The anchored column
  picker is the exception: `dialog` role, auto-focus, and Escape, but no
  Tab trap and no `aria-modal` (`[appModalSheetModal]="false"`). Focus
  returns to the opener on close — unless another surface already claimed
  it, in which case the restore stands down.
- `:focus-visible` rings on every interactive element (accent red, 2px, offset 2)
- `prefers-reduced-motion` respected globally
- Segmented controls are `role="radiogroup"`/`radio` + `aria-checked`
  (main-content, preferences, settings-audio); the sidebar is a `tree`;
  artists a `listbox`; the Settings strip is a `tablist`/`tab`/`tabpanel`;
  context menus and menu-bar dropdowns are `menu`/`menuitem`. All carry the
  full keyboard model — one tab stop (roving `tabindex`, on the
  checked/selected item), arrows move focus and selection, Home/End jump,
  disabled items are skipped (`appRovingFocus`, per-menu-level inside
  flyouts) — and context-menu items use `menuitemcheckbox` +
  `aria-checked`, `aria-disabled` (never the `disabled` attribute, which
  removes them from the a11y tree), `aria-haspopup`/`aria-expanded` on
  submenu parents, focus moving into an opened flyout and back on close.
  Menu-bar triggers carry `aria-haspopup="menu"`/`aria-expanded` and
  return focus to the trigger on close.
- Touch targets are desktop-sized by design (pointer-only app, no mobile target)

## Last updated

2026-09-20 — created. Captures the shipped graphite/red system after an
accessibility + integrity audit: restored the orphaned Settings sheet,
retuned muted/accent text tokens to clear AA, gave every modal dialog
semantics and a focus trap, added reduced-motion support and track-list
empty/loading states.
2026-09-20 — completed the composite-widget keyboard model
(`appRovingFocus`): arrows/Home/End with roving tabindex on the
segmented controls, artist listbox, context menu, and menu bar;
`menuitemcheckbox`/`aria-haspopup`/`aria-expanded` on context-menu items;
menu focus save/restore.
