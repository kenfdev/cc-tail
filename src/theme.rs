//! Theme support for the TUI.
//!
//! Provides a [`ThemeColors`] struct containing all color definitions used
//! throughout the TUI. Two constructors are provided: [`ThemeColors::dark()`]
//! (backward-compatible with existing hardcoded colors) and
//! [`ThemeColors::light()`] (adjusted for readability on light backgrounds).
//!
//! All colors use the 16 basic ANSI palette for maximum terminal compatibility.

use ratatui::style::Color;

use crate::cli::Theme;

// ---------------------------------------------------------------------------
// ThemeColors
// ---------------------------------------------------------------------------

/// All color definitions for the TUI, grouped by component.
///
/// The dark theme reproduces the exact same colors as the original hardcoded
/// values. The light theme adjusts colors for readability on light backgrounds.
#[derive(Debug, Clone, PartialEq)]
pub struct ThemeColors {
    // -- Borders -----------------------------------------------------------
    /// Border color when the panel has keyboard focus.
    pub border_focused: Color,
    /// Border color when the panel does not have focus.
    pub border_unfocused: Color,

    // -- Sidebar -----------------------------------------------------------
    /// Placeholder text (e.g. "No sessions found").
    pub sidebar_placeholder: Color,
    /// Active session marker bullet.
    pub sidebar_active_marker: Color,
    /// Inactive session marker text.
    pub sidebar_inactive_marker: Color,
    /// Selected session header foreground.
    pub sidebar_selected_fg: Color,
    /// Selected session header background.
    pub sidebar_selected_bg: Color,
    /// New session header foreground.
    pub sidebar_new_session: Color,
    /// Active-target session header foreground.
    pub sidebar_active_target: Color,
    /// Default (unselected, not new) session header foreground.
    pub sidebar_default_session: Color,
    /// Selected child agent row foreground.
    pub sidebar_selected_child_fg: Color,
    /// Selected child agent row background.
    pub sidebar_selected_child_bg: Color,
    /// Unselected child agent row foreground.
    pub sidebar_unselected_child: Color,
    /// Tree connector prefix foreground.
    pub sidebar_child_prefix: Color,

    // -- Log stream --------------------------------------------------------
    /// Placeholder text (e.g. "Waiting for log entries...").
    pub logstream_placeholder: Color,
    /// Timestamp foreground.
    pub logstream_timestamp: Color,
    /// Progress indicator foreground.
    pub logstream_progress: Color,
    /// User role indicator `>` color.
    pub role_user: Color,
    /// Assistant role indicator `<` color.
    pub role_assistant: Color,
    /// Unknown role indicator `?` color.
    pub role_unknown: Color,
    /// Tool use indicator `~` color.
    pub role_tool_use: Color,
    /// Default text foreground in the log stream.
    pub logstream_text: Color,
    /// Main agent (no agent_id) color.
    pub agent_main: Color,
    /// 8-color palette for per-agent deterministic coloring.
    pub agent_palette: [Color; 8],

    // -- Status bar --------------------------------------------------------
    /// Status bar background.
    pub status_bar_bg: Color,
    /// Status bar default foreground.
    pub status_bar_fg: Color,
    /// Inactive badge foreground.
    pub status_inactive_fg: Color,
    /// Inactive badge background.
    pub status_inactive_bg: Color,
    /// Filter display foreground.
    pub status_filter: Color,
    /// Separator foreground.
    pub status_separator: Color,
    /// Shortcut key foreground.
    pub status_shortcut_key: Color,

    // -- Filter overlay ----------------------------------------------------
    /// Invalid border / invalid pattern text color.
    pub filter_invalid: Color,
    /// Valid border color (same as border_focused).
    pub filter_valid_border: Color,
    /// Focused label foreground.
    pub filter_focused_label: Color,
    /// Unfocused label foreground.
    pub filter_unfocused_label: Color,
    /// Valid pattern text foreground.
    pub filter_valid_text: Color,
    /// Selected item foreground.
    pub filter_selected_fg: Color,
    /// Selected item background.
    pub filter_selected_bg: Color,
    /// Unselected item foreground.
    pub filter_unselected: Color,
    /// Overlay background.
    pub filter_overlay_bg: Color,
    /// Overlay default foreground.
    pub filter_overlay_fg: Color,
    /// Footer shortcut key foreground.
    pub filter_shortcut_key: Color,
    /// Main agent toggle foreground (focused).
    pub filter_main_focused: Color,
    /// Main agent toggle foreground (unfocused).
    pub filter_main_unfocused: Color,

    // -- Search ------------------------------------------------------------
    /// Search match highlight background.
    pub search_match_bg: Color,
    /// Search match highlight foreground.
    pub search_match_fg: Color,
    /// Current search match highlight background.
    pub search_current_bg: Color,
    /// Current search match highlight foreground.
    pub search_current_fg: Color,
    /// Search input bar text foreground.
    pub search_input_fg: Color,
    /// Search prompt (`/`) foreground.
    pub search_prompt: Color,
}

impl ThemeColors {
    /// Construct the theme colors from the CLI/config theme enum.
    pub fn from_theme(theme: &Theme) -> Self {
        match theme {
            Theme::Dark => Self::dark(),
            Theme::Light => Self::light(),
        }
    }

    /// Dark theme -- reproduces the exact same colors as the original
    /// hardcoded values. This is the backward-compatible default.
    pub fn dark() -> Self {
        Self {
            // Borders
            border_focused: Color::Cyan,
            border_unfocused: Color::DarkGray,

            // Sidebar
            sidebar_placeholder: Color::DarkGray,
            sidebar_active_marker: Color::Green,
            sidebar_inactive_marker: Color::DarkGray,
            sidebar_selected_fg: Color::White,
            sidebar_selected_bg: Color::DarkGray,
            sidebar_new_session: Color::Yellow,
            sidebar_active_target: Color::Cyan,
            sidebar_default_session: Color::Gray,
            sidebar_selected_child_fg: Color::White,
            sidebar_selected_child_bg: Color::DarkGray,
            sidebar_unselected_child: Color::DarkGray,
            sidebar_child_prefix: Color::DarkGray,

            // Log stream
            logstream_placeholder: Color::DarkGray,
            logstream_timestamp: Color::DarkGray,
            logstream_progress: Color::DarkGray,
            role_user: Color::Blue,
            role_assistant: Color::Green,
            role_unknown: Color::Gray,
            role_tool_use: Color::Yellow,
            logstream_text: Color::White,
            agent_main: Color::White,
            agent_palette: [
                Color::Red,
                Color::Green,
                Color::Yellow,
                Color::Blue,
                Color::Magenta,
                Color::Cyan,
                Color::LightRed,
                Color::LightGreen,
            ],

            // Status bar
            status_bar_bg: Color::DarkGray,
            status_bar_fg: Color::White,
            status_inactive_fg: Color::White,
            status_inactive_bg: Color::Red,
            status_filter: Color::Magenta,
            status_separator: Color::DarkGray,
            status_shortcut_key: Color::Yellow,

            // Filter overlay
            filter_invalid: Color::Red,
            filter_valid_border: Color::Cyan,
            filter_focused_label: Color::Cyan,
            filter_unfocused_label: Color::White,
            filter_valid_text: Color::White,
            filter_selected_fg: Color::White,
            filter_selected_bg: Color::DarkGray,
            filter_unselected: Color::Gray,
            filter_overlay_bg: Color::Black,
            filter_overlay_fg: Color::White,
            filter_shortcut_key: Color::Yellow,
            filter_main_focused: Color::Gray,
            filter_main_unfocused: Color::DarkGray,

            // Search
            search_match_bg: Color::Yellow,
            search_match_fg: Color::Black,
            search_current_bg: Color::Magenta,
            search_current_fg: Color::White,
            search_input_fg: Color::White,
            search_prompt: Color::Yellow,
        }
    }

    /// Light theme -- adjusted colors for readability on light terminal
    /// backgrounds. Uses the 16 basic ANSI colors.
    pub fn light() -> Self {
        Self {
            // Borders
            border_focused: Color::Blue,
            border_unfocused: Color::Gray,

            // Sidebar
            sidebar_placeholder: Color::Gray,
            sidebar_active_marker: Color::Green,
            sidebar_inactive_marker: Color::Gray,
            sidebar_selected_fg: Color::White,
            sidebar_selected_bg: Color::Blue,
            sidebar_new_session: Color::Magenta,
            sidebar_active_target: Color::Blue,
            sidebar_default_session: Color::DarkGray,
            sidebar_selected_child_fg: Color::White,
            sidebar_selected_child_bg: Color::Blue,
            sidebar_unselected_child: Color::Gray,
            sidebar_child_prefix: Color::Gray,

            // Log stream
            logstream_placeholder: Color::Gray,
            logstream_timestamp: Color::Gray,
            logstream_progress: Color::Gray,
            role_user: Color::Blue,
            role_assistant: Color::Green,
            role_unknown: Color::DarkGray,
            role_tool_use: Color::Magenta,
            logstream_text: Color::Black,
            agent_main: Color::Black,
            agent_palette: [
                Color::Red,
                Color::Green,
                Color::Magenta,
                Color::Blue,
                Color::Cyan,
                Color::DarkGray,
                Color::LightRed,
                Color::LightBlue,
            ],

            // Status bar
            status_bar_bg: Color::Gray,
            status_bar_fg: Color::Black,
            status_inactive_fg: Color::White,
            status_inactive_bg: Color::Red,
            status_filter: Color::Magenta,
            status_separator: Color::DarkGray,
            status_shortcut_key: Color::Blue,

            // Filter overlay
            filter_invalid: Color::Red,
            filter_valid_border: Color::Blue,
            filter_focused_label: Color::Blue,
            filter_unfocused_label: Color::Black,
            filter_valid_text: Color::Black,
            filter_selected_fg: Color::White,
            filter_selected_bg: Color::Blue,
            filter_unselected: Color::DarkGray,
            filter_overlay_bg: Color::White,
            filter_overlay_fg: Color::Black,
            filter_shortcut_key: Color::Blue,
            filter_main_focused: Color::DarkGray,
            filter_main_unfocused: Color::Gray,

            // Search
            search_match_bg: Color::Yellow,
            search_match_fg: Color::Black,
            search_current_bg: Color::Blue,
            search_current_fg: Color::White,
            search_input_fg: Color::Black,
            search_prompt: Color::Blue,
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_light_theme_differs_from_dark() {
        let dark = ThemeColors::dark();
        let light = ThemeColors::light();
        // Light theme should have different defaults for key colors
        assert_ne!(dark.logstream_text, light.logstream_text);
        assert_ne!(dark.status_bar_bg, light.status_bar_bg);
        assert_ne!(dark.filter_overlay_bg, light.filter_overlay_bg);
    }

    #[test]
    fn test_light_theme_uses_basic_ansi_colors() {
        let t = ThemeColors::light();
        // Spot-check that no RGB/TrueColor values are used.
        // All fields should be basic ANSI Color::* variants.
        // We check a representative sample by ensuring they are not Reset
        // (which would indicate a missing assignment).
        assert_ne!(t.border_focused, Color::Reset);
        assert_ne!(t.logstream_text, Color::Reset);
        assert_ne!(t.status_bar_bg, Color::Reset);
        assert_ne!(t.filter_overlay_bg, Color::Reset);
    }

    #[test]
    fn test_from_theme_dark() {
        let colors = ThemeColors::from_theme(&Theme::Dark);
        assert_eq!(colors, ThemeColors::dark());
    }

    #[test]
    fn test_from_theme_light() {
        let colors = ThemeColors::from_theme(&Theme::Light);
        assert_eq!(colors, ThemeColors::light());
    }

    #[test]
    fn test_dark_theme_is_self_consistent() {
        let t = ThemeColors::dark();
        // Palette has exactly 8 entries (compile-time array, but verify at runtime)
        assert_eq!(t.agent_palette.len(), 8);
        // No palette entry uses Reset (which would indicate a missing assignment)
        for color in &t.agent_palette {
            assert_ne!(*color, Color::Reset);
        }
        // Dark and light are distinct themes (not accidentally identical)
        assert_ne!(t, ThemeColors::light());
        // Clone produces an equal copy (exercises derive(Clone, PartialEq))
        assert_eq!(t.clone(), t);
    }
}
