# Brief R4: Refinement pass 4, Home in the middle, and a start screen (2.0)

From the owner (2026-11-20), after R3 (afbe8e1): "a setting that lets you pick what screen to start on … make home … the center of the slider. That way your main page is the middle, and both other options are a clearly defined left or right swipe away."

## Decisions (architect, following the owner)
- **The order becomes Search · Home · Library**, fixed, with Home in the middle. Every root is then one swipe or one step from Home.
- The order does **not** change with the start screen. Tabs that move around break muscle memory and would flip the swipe's directions. The owner asked "Home (or maybe the preferred starting screen)"; the report states this choice, and it is cheap to revisit.
- **Start screen** is a Preferences setting with the choices Home (default), Search and Library. It applies at app start, and also when the source changes if that resets the navigation today.
  - Library opens at its remembered section, as a tab click does.
  - Search opens with the field focused.

## Scope
1. **Navigation order.** `lib/navModel.ts` and `state/nav.ts`, the top bar's pill and its indicator, and the swipe's root order (`nav.activeRoot()` and `selectRoot`) all follow Search · Home · Library.
   - Keyboard shortcuts for the roots keep their meaning by name, not by position. If a shortcut is positional (1/2/3), make it follow the visual order and update the shortcut help.
   - The roving focus and the arrow keys in the pill follow the visual order.
2. **Start screen setting.**
   - `settings.toml` gets a new key (e.g. `startScreen` = `home` | `search` | `library`) through the prefs module, in the same way as `libraryView`: a command `prefs_set_start_screen` with its capability and permission, and validation. An unknown value reads as Home.
   - The init path uses it for the first root before the first paint where the code allows, so the app never flashes Home first.
   - Preferences gets a small "Start on" control: a segmented control or a select, in the existing General or Library card, whichever fits.
   - Onboarding is unchanged: the first run still ends on Home.
3. **Swipe.** Nothing changes beyond the order. The ends are now Search on the left and Library on the right, and Home is one swipe from both.

## Acceptance
- The pill reads Search · Home · Library. A left swipe from Home goes to Library and a right swipe goes to Search (the direction convention unchanged from R1: a left swipe means next).
- The start setting persists, is validated and is applied at launch without a flash.
- Tests:
  - the prefs round trip and its validation (Rust);
  - the order and the next/previous root helpers (node).
- Checks (report commands): `pnpm build`, `pnpm check:colors`; `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings` (and `--all-features`), `cargo test --workspace --all-targets`.
- Docs:
  - `docs/DESIGN.md`: Top bar, Preferences, the swipe.
  - `docs/ARCHITECTURE.md`.
  - `README.md`.
  - `scripts/smoke.md`: a "2.0 R4" section.
  - `docs/PLAN.md`.
- Do not commit; report changed files, decisions, checks, open items. This pnpm does not accept `-s`. No GUI windows.
