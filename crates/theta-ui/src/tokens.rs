//! Shared dimensions so every panel has the same density (Zed-like: compact
//! rows, 1px borders, 12px chrome text).

use gpui_kit::{Pixels, px};

pub const TITLE_BAR_HEIGHT: Pixels = px(34.);
pub const TAB_BAR_HEIGHT: Pixels = px(32.);
pub const STATUS_BAR_HEIGHT: Pixels = px(26.);
pub const PANEL_HEADER_HEIGHT: Pixels = px(28.);
pub const ROW_HEIGHT: Pixels = px(24.);
/// Square size of toolbar ghost buttons.
pub const TOOL_BUTTON_SIZE: Pixels = px(22.);
/// Font size of chrome text (tabs, status bar, panel headers, row details).
pub const CHROME_TEXT_SIZE: Pixels = px(12.);
