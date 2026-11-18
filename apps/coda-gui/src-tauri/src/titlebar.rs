//! macOS: no visible title bar. The window keeps its native frame
//! (rounded corners, shadow, resizing, full screen) and its close,
//! minimise and zoom buttons, but the title bar is an overlay with no
//! title, so the webview fills the whole window. The buttons are inset
//! into the sidebar's top row (`--titlebar-h` in `app.css`: keep the two
//! in step); the page and panel headers are the drag regions. wry puts
//! the buttons back at this inset whenever the window's content redraws
//! (after a resize or leaving full screen).

use tauri::LogicalPosition;

/// The buttons' inset from the window's top-left corner, in points: `x`
/// to the close button's left edge, `y` sizes the title bar area AppKit
/// lays them out in (they end up vertically centred in a ~40pt row).
const BUTTONS: LogicalPosition<f64> = LogicalPosition::new(18.0, 20.0);

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
