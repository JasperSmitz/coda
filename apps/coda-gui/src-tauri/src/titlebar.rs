//! macOS: no visible title bar. The window keeps its native frame
//! (rounded corners, shadow, resizing, full screen) and its close,
//! minimise and zoom buttons, but the title bar is an overlay with no
//! title, so the webview fills the whole window. The buttons are inset
//! into the top bar's row, vertically centred on its pill (`--topbar-top`,
//! the pill's 50px and `--window-buttons-w` in `tokens.css`/`app.css`:
//! keep them in step);
//! the top bar is the drag region. wry puts the buttons back at this inset
//! whenever the window's content redraws (after a resize or leaving full
//! screen).

use tauri::LogicalPosition;

/// The buttons' inset from the window's top-left corner, in points: `x`
/// to the close button's left edge. `y` sets the title bar area AppKit
/// lays them out in (tao makes it the button's height plus `y` and moves
/// the buttons only sideways); in practice their centre ends up at about
/// `y` from the top (1.x: y = 20 centred them in a 40pt row; 2.0 N1:
/// 26 on a 52px bar). 41 centres them on the navigation pill, whose top
/// is `--topbar-top` (16px) down and which is 50px tall (2.0 R1); to be
/// confirmed by the owner on a Mac (`scripts/smoke.md` "2.0 R1").
const BUTTONS: LogicalPosition<f64> = LogicalPosition::new(18.0, 41.0);

/// The main window's builder, with the title bar overlaid, its title
/// hidden and the buttons inset.
pub fn style<R: tauri::Runtime, M: tauri::Manager<R>>(
    builder: tauri::WebviewWindowBuilder<'_, R, M>,
) -> tauri::WebviewWindowBuilder<'_, R, M> {
    builder
        .title_bar_style(tauri::TitleBarStyle::Overlay)
        .hidden_title(true)
        .traffic_light_position(BUTTONS)
}
