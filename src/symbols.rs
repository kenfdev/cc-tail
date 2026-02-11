//! Symbol set for TUI rendering.
//!
//! Unicode mode uses prettier glyphs; ASCII mode uses basic characters
//! for better compatibility with terminals that lack Unicode support.

// ---------------------------------------------------------------------------
// Symbols struct
// ---------------------------------------------------------------------------

/// Symbol set for TUI rendering.
///
/// Unicode mode uses prettier glyphs; ASCII mode uses basic characters.
#[derive(Debug, Clone)]
pub struct Symbols {
    /// Active session marker in sidebar (Unicode: `\u{25cf}` / ASCII: `*`)
    pub active_marker: &'static str,
    /// Tree connector for child agents (Unicode: `\u{2514}` / ASCII: `` `- ``)
    pub tree_connector: &'static str,
    /// Progress/play indicator (Unicode: `\u{25b6}` / ASCII: `>`)
    pub progress_indicator: &'static str,
    /// Search cursor block (Unicode: `\u{2588}` / ASCII: `_`)
    pub search_cursor: &'static str,
}

impl Symbols {
    /// Create a new `Symbols` based on the mode flag.
    ///
    /// When `ascii_mode` is `true`, returns ASCII-safe characters.
    /// When `false`, returns Unicode glyphs.
    pub fn new(ascii_mode: bool) -> Self {
        if ascii_mode {
            Self::ascii()
        } else {
            Self::unicode()
        }
    }

    /// Unicode symbol set.
    pub fn unicode() -> Self {
        Self {
            active_marker: "\u{25cf}",     // ●
            tree_connector: "\u{2514}",    // └
            progress_indicator: "\u{25b6}", // ▶
            search_cursor: "\u{2588}",     // █
        }
    }

    /// ASCII-safe symbol set.
    pub fn ascii() -> Self {
        Self {
            active_marker: "*",
            tree_connector: "`-",
            progress_indicator: ">",
            search_cursor: "_",
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
    fn test_symbols_new_false_is_unicode() {
        let s = Symbols::new(false);
        assert_eq!(s.active_marker, "\u{25cf}");
        assert_eq!(s.tree_connector, "\u{2514}");
        assert_eq!(s.progress_indicator, "\u{25b6}");
        assert_eq!(s.search_cursor, "\u{2588}");
    }

    #[test]
    fn test_symbols_new_true_is_ascii() {
        let s = Symbols::new(true);
        assert_eq!(s.active_marker, "*");
        assert_eq!(s.tree_connector, "`-");
        assert_eq!(s.progress_indicator, ">");
        assert_eq!(s.search_cursor, "_");
    }
}
