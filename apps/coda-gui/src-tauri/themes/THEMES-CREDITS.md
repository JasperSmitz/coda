# Built-in themes: origins and licences

The built-in themes are compiled into the app (`src-tauri/src/theme/sources.rs`). The Ghostty-format files in this folder are named after the theme, as Ghostty names them. Each has a `coda-accent` line added: Ghostty ignores it, and Coda reads it as the accent colour.

| Theme | Origin | Licence |
|---|---|---|
| Coda, Coda Light | This app: its original dark and light looks, written as themes. | MIT (this project) |
| Kanagawabones | Ghostty's bundled theme file (from the iTerm2-Color-Schemes collection, MIT). The scheme is from zenbones.nvim by Michael Chris Lopez (MIT), based on Kanagawa by rebelot (MIT). | MIT |
| Kanso Zen, Kanso Pearl | Ghostty's bundled theme files (iTerm2-Color-Schemes, MIT). The scheme is kanso.nvim by webhooked (MIT). | MIT |
| Catppuccin Mocha, Catppuccin Latte | Ghostty's bundled theme files (iTerm2-Color-Schemes, MIT). The scheme is Catppuccin (catppuccin.com, MIT). | MIT |

The palette, background, foreground, cursor and selection colours are copied unchanged from Ghostty 1.x's `Ghostty.app/Contents/Resources/ghostty/themes/`. The accent colours were picked by the owner: Kanagawabones `#e5c283`, Kanso Zen `#8ba4b0`, Kanso Pearl `#4d699b`, Catppuccin Mocha `#cba6f7` (mauve) and Catppuccin Latte `#8839ef` (mauve).
