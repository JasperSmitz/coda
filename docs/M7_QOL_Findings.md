# M7 — Keybindings & QoL findings

Owner notes from daily use, collected before M7 was scoped. **Addressed in M7** (brief: `docs/plans/m7-keybindings-qol.md`); the key table below is the pre-M7 keymap, kept as a record. For current keys see the README.

## Already on the list

- Play a whole album or playlist directly (not only enqueue). Ideas so far: move navigation off Enter, or Shift+Enter plays the hovered item. Shift+Enter needs the kitty keyboard protocol, so it needs a fallback key.
- Provisional keys to settle: `,` settings, `S` save queue as playlist, `P` add to playlist, `N` / `e` / `D D` playlist new / rename / delete, `w` save order. `l` is currently unbound.

## Current keys (for reference)

| Key | Action | Keep / change / notes |
| --- | --- | --- |
| `j` `k` `↓` `↑` | Move selection | |
| `Enter` | Open / play now / play entry | |
| `h` `Backspace` | Go up (`Esc` leaves results) | |
| `a` / `A` | Add / play next | |
| `1` / `2` | Local / Navidrome library | |
| `/` | Search | |
| `r` / `o` | Refresh (Navidrome) / sort albums | |
| `d` / `J` `K` | Queue: remove / move down, up | |
| `g` / `s` | Queue: select current / shuffle | |
| `C C` / `S` | Queue: clear / save as playlist | |
| `P` | Add to a Navidrome playlist | |
| `N` / `e` / `D D` | Playlists: new / rename / delete | |
| `d d` / `J` `K` | Playlist: remove / move song | |
| `w` / `Esc` | Playlist: save / discard order | |
| `R` / `v` | Repeat (off/all/one) / visualizer | |
| `Space` `n` / `p` | Pause / next / previous | |
| `←` `→` / `+` `-` | Seek ±5 s / volume | |
| `Tab` | Switch panel | |
| `,` | Settings | |
| `?` | Help | |
| `q` `Ctrl-C` | Quit | |

## Keybindings — what feels clunky

- Less about feeling clunky, more about feeling logical. Next and Previous being n and p makes sense, but I think it would be nicer if they were next to each other, like n and m. Similarly I'd like to make more use of shift or control as a modifier. a and A make so much sense and I think there are others that can be "merged" like that.
- I've been considering we could maybe change navigation. j/k and up and down can remain the navigational buttons vertically, but how about l or -> moves in, <- or h moves out. That would allow enter to take the role of a simple play all button.

## Missing actions — what I reached for and couldn't do

- Playing a whole album. I'd love to be able to hit play on an album and it plays the album, rather than queueing up the album (or having to hit tab and C C to clear the entire queue before queueing up the album).
- I think similarly for playlists and honestly albums too (though I like listening to albums in order) there should be a "Play Shuffled" button.

## Visualizer

Does it feel in sync? Anything off about look, size or behaviour?

- Feels perfect, the only "complaint" is that on fullscreen the visualizer is kind of tucked off into the bottom left corner when I feel it could or should spread across the available width, but with the limited bands I'm not sure if that would look good. What came to mind is artificially expanding the bands, we could theoretically double the bands and just have the fake in between bands be the "average" of the two real bands next to it.

## Bugs / odd behaviour

What happened, what you expected, and local or Navidrome.

- No bugs found.

## Other ideas

-
