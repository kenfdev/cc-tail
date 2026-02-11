//! Search state machine, matching engine, and navigation logic.
//!
//! The search feature has three states:
//! - **Inactive**: No search active. `/` transitions to Input.
//! - **Input**: User is typing a search query. `Enter` confirms, `Esc` cancels.
//! - **Active**: Matches are highlighted. `n`/`N` navigate. `Esc` clears.
//!
//! Matching is case-insensitive plain text substring (no regex).
//! Only operates on visible (filtered) entries.

// ---------------------------------------------------------------------------
// Search mode enum
// ---------------------------------------------------------------------------

/// The current state of the search feature.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum SearchMode {
    /// No search active.
    #[default]
    Inactive,
    /// User is typing a search query in the input bar.
    Input,
    /// Search is confirmed; matches are highlighted and navigable.
    Active,
}

// ---------------------------------------------------------------------------
// Search match
// ---------------------------------------------------------------------------

/// A single match occurrence within the rendered output.
///
/// `line_index` is the index into the flat list of rendered `Line`s.
/// `byte_start` and `byte_len` describe the match position within the
/// concatenated text content of that line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchMatch {
    /// Index into the flat rendered lines vector.
    pub line_index: usize,
    /// Byte offset within the line's concatenated text.
    pub byte_start: usize,
    /// Length of the match in bytes.
    pub byte_len: usize,
}

// ---------------------------------------------------------------------------
// Search state
// ---------------------------------------------------------------------------

/// Complete search state, owned by `App`.
#[derive(Debug, Clone)]
pub struct SearchState {
    /// Current search mode.
    pub mode: SearchMode,
    /// The input buffer while the user is typing (Input mode).
    pub input_buffer: String,
    /// The confirmed search query (set on Enter, cleared on cancel).
    pub query: String,
    /// All match occurrences found in the current rendered output.
    pub matches: Vec<SearchMatch>,
    /// Index of the currently highlighted match (for `n`/`N` navigation).
    pub current_match_index: Option<usize>,
}

impl Default for SearchState {
    fn default() -> Self {
        Self {
            mode: SearchMode::Inactive,
            input_buffer: String::new(),
            query: String::new(),
            matches: Vec::new(),
            current_match_index: None,
        }
    }
}

impl SearchState {
    /// Transition from Inactive to Input mode.
    ///
    /// Clears the input buffer but preserves any previous query
    /// (so pressing Enter immediately re-searches the last query).
    pub fn start_input(&mut self) {
        self.mode = SearchMode::Input;
        self.input_buffer.clear();
    }

    /// Append a character to the input buffer (Input mode only).
    pub fn on_char(&mut self, ch: char) {
        if self.mode == SearchMode::Input {
            self.input_buffer.push(ch);
        }
    }

    /// Remove the last character from the input buffer (Input mode only).
    pub fn on_backspace(&mut self) {
        if self.mode == SearchMode::Input {
            self.input_buffer.pop();
        }
    }

    /// Confirm the search query (Enter in Input mode).
    ///
    /// If the input buffer is non-empty, uses it as the new query.
    /// If empty and a previous query exists, re-uses the previous query.
    /// Transitions to Active mode if a query is available, otherwise
    /// transitions back to Inactive.
    pub fn confirm(&mut self) {
        if self.mode != SearchMode::Input {
            return;
        }

        if !self.input_buffer.is_empty() {
            self.query = self.input_buffer.clone();
        }

        if self.query.is_empty() {
            // No query to search for; go back to inactive.
            self.mode = SearchMode::Inactive;
        } else {
            self.mode = SearchMode::Active;
            // Matches will be computed by the caller (App/UI).
            self.matches.clear();
            self.current_match_index = None;
        }

        self.input_buffer.clear();
    }

    /// Cancel the search (Escape key).
    ///
    /// From Input mode: returns to Inactive, discards input buffer.
    /// From Active mode: clears everything and returns to Inactive.
    pub fn cancel(&mut self) {
        match self.mode {
            SearchMode::Input => {
                self.mode = SearchMode::Inactive;
                self.input_buffer.clear();
            }
            SearchMode::Active => {
                self.mode = SearchMode::Inactive;
                self.input_buffer.clear();
                self.query.clear();
                self.matches.clear();
                self.current_match_index = None;
            }
            SearchMode::Inactive => {}
        }
    }

    /// Navigate to the next match.
    ///
    /// Wraps around from the last match to the first.
    pub fn next_match(&mut self) {
        if self.matches.is_empty() {
            return;
        }
        match self.current_match_index {
            Some(idx) => {
                self.current_match_index = Some((idx + 1) % self.matches.len());
            }
            None => {
                self.current_match_index = Some(0);
            }
        }
    }

    /// Navigate to the previous match.
    ///
    /// Wraps around from the first match to the last.
    pub fn prev_match(&mut self) {
        if self.matches.is_empty() {
            return;
        }
        match self.current_match_index {
            Some(idx) => {
                if idx == 0 {
                    self.current_match_index = Some(self.matches.len() - 1);
                } else {
                    self.current_match_index = Some(idx - 1);
                }
            }
            None => {
                self.current_match_index = Some(self.matches.len() - 1);
            }
        }
    }

    /// Format the match counter for display (e.g. `"[3/17]"`).
    ///
    /// Returns `None` if not in Active mode or no query is set.
    pub fn match_counter_display(&self) -> Option<String> {
        if self.mode != SearchMode::Active {
            return None;
        }

        if self.matches.is_empty() {
            return Some("[0/0]".to_string());
        }

        match self.current_match_index {
            Some(idx) => Some(format!("[{}/{}]", idx + 1, self.matches.len())),
            None => Some(format!("[0/{}]", self.matches.len())),
        }
    }

    /// Returns true if search is in Active mode.
    pub fn is_active(&self) -> bool {
        self.mode == SearchMode::Active
    }

    /// Returns true if search is in Input mode.
    pub fn is_input(&self) -> bool {
        self.mode == SearchMode::Input
    }

    /// Returns the line index of the current match, if any.
    pub fn current_match_line(&self) -> Option<usize> {
        self.current_match_index
            .and_then(|idx| self.matches.get(idx))
            .map(|m| m.line_index)
    }
}

// ---------------------------------------------------------------------------
// Matching engine
// ---------------------------------------------------------------------------

/// Find all non-overlapping, case-insensitive substring matches.
///
/// Returns a vector of `(byte_start, byte_len)` tuples whose offsets refer
/// to the **original** `text` (not the lowercased copy). This is critical
/// because some characters change byte length when lowercased (e.g.
/// Turkish İ U+0130, German ẞ U+1E9E), and callers slice the original text
/// using these offsets.
///
/// Matches are found left-to-right, advancing past each match to avoid
/// overlaps (standard vim/less behavior).
pub fn find_matches(text: &str, query: &str) -> Vec<(usize, usize)> {
    if query.is_empty() {
        return Vec::new();
    }

    let text_lower = text.to_lowercase();
    let query_lower = query.to_lowercase();
    let query_len = query_lower.len(); // byte length in lowercased space

    // Build a mapping from lowercased byte offsets to original byte offsets.
    // For each original character we record (lower_byte_offset, orig_byte_offset).
    // We also add a sentinel entry for the end-of-string positions.
    let lower_to_orig = build_lower_to_orig_map(text);

    let mut results = Vec::new();
    let mut start = 0; // byte offset in the lowercased string

    while start + query_len <= text_lower.len() {
        if let Some(pos) = text_lower[start..].find(&query_lower) {
            let lower_start = start + pos;
            let lower_end = lower_start + query_len;

            // Map the lowercased byte range back to the original string.
            let orig_start = map_lower_to_orig(&lower_to_orig, lower_start, text.len());
            let orig_end = map_lower_to_orig(&lower_to_orig, lower_end, text.len());
            let orig_len = orig_end - orig_start;

            results.push((orig_start, orig_len));
            start = lower_end; // advance past match (non-overlapping)
        } else {
            break;
        }
    }

    results
}

/// Build a mapping from lowercased byte positions to original byte positions.
///
/// Returns a sorted `Vec<(lower_byte_offset, orig_byte_offset)>` with one
/// entry per character plus a sentinel for the end of the string.
fn build_lower_to_orig_map(text: &str) -> Vec<(usize, usize)> {
    let mut map = Vec::new();
    let mut lower_offset: usize = 0;
    let mut orig_offset: usize = 0;

    for ch in text.chars() {
        map.push((lower_offset, orig_offset));
        let lower_len: usize = ch.to_lowercase().map(|c| c.len_utf8()).sum();
        lower_offset += lower_len;
        orig_offset += ch.len_utf8();
    }

    // Sentinel for end-of-string.
    map.push((lower_offset, orig_offset));
    map
}

/// Look up the original byte offset for a given lowercased byte offset.
///
/// Uses binary search on the mapping. If the offset falls exactly on a
/// character boundary it returns the corresponding original offset.
/// If it falls mid-character (which shouldn't happen with well-formed
/// match positions), it returns the start of the containing character.
fn map_lower_to_orig(map: &[(usize, usize)], lower_pos: usize, orig_len: usize) -> usize {
    match map.binary_search_by_key(&lower_pos, |&(lo, _)| lo) {
        Ok(idx) => map[idx].1,
        Err(idx) => {
            // lower_pos is between map[idx-1] and map[idx].
            // Return the original offset of the preceding character boundary.
            if idx == 0 {
                0
            } else if idx >= map.len() {
                orig_len
            } else {
                map[idx - 1].1
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- find_matches tests --------------------------------------------------

    #[test]
    fn test_find_matches_basic() {
        let matches = find_matches("hello world hello", "hello");
        assert_eq!(matches, vec![(0, 5), (12, 5)]);
    }

    #[test]
    fn test_find_matches_case_insensitive() {
        let matches = find_matches("Hello HELLO hElLo", "hello");
        assert_eq!(matches, vec![(0, 5), (6, 5), (12, 5)]);
    }

    #[test]
    fn test_find_matches_empty_query() {
        let matches = find_matches("hello", "");
        assert!(matches.is_empty());
    }

    #[test]
    fn test_find_matches_empty_text() {
        let matches = find_matches("", "hello");
        assert!(matches.is_empty());
    }

    #[test]
    fn test_find_matches_no_match() {
        let matches = find_matches("hello world", "xyz");
        assert!(matches.is_empty());
    }

    #[test]
    fn test_find_matches_non_overlapping() {
        // "aaa" searching for "aa" should find only one match (non-overlapping)
        let matches = find_matches("aaa", "aa");
        assert_eq!(matches, vec![(0, 2)]);
    }

    #[test]
    fn test_find_matches_query_longer_than_text() {
        let matches = find_matches("hi", "hello world");
        assert!(matches.is_empty());
    }

    #[test]
    fn test_find_matches_exact_match() {
        let matches = find_matches("hello", "hello");
        assert_eq!(matches, vec![(0, 5)]);
    }

    #[test]
    fn test_find_matches_single_char() {
        let matches = find_matches("banana", "a");
        assert_eq!(matches, vec![(1, 1), (3, 1), (5, 1)]);
    }

    #[test]
    fn test_find_matches_adjacent() {
        let matches = find_matches("abab", "ab");
        assert_eq!(matches, vec![(0, 2), (2, 2)]);
    }

    // -- SearchState state machine tests ------------------------------------

    #[test]
    fn test_default_state_is_inactive() {
        let state = SearchState::default();
        assert_eq!(state.mode, SearchMode::Inactive);
        assert!(state.input_buffer.is_empty());
        assert!(state.query.is_empty());
        assert!(state.matches.is_empty());
        assert!(state.current_match_index.is_none());
    }

    #[test]
    fn test_start_input_transitions_to_input_mode() {
        let mut state = SearchState::default();
        state.start_input();
        assert_eq!(state.mode, SearchMode::Input);
        assert!(state.input_buffer.is_empty());
    }

    #[test]
    fn test_on_char_appends_to_buffer() {
        let mut state = SearchState::default();
        state.start_input();
        state.on_char('h');
        state.on_char('e');
        state.on_char('l');
        assert_eq!(state.input_buffer, "hel");
    }

    #[test]
    fn test_on_char_noop_when_not_input_mode() {
        let mut state = SearchState::default();
        state.on_char('h');
        assert!(state.input_buffer.is_empty());
    }

    #[test]
    fn test_on_backspace_removes_last_char() {
        let mut state = SearchState::default();
        state.start_input();
        state.on_char('h');
        state.on_char('e');
        state.on_backspace();
        assert_eq!(state.input_buffer, "h");
    }

    #[test]
    fn test_on_backspace_empty_buffer_noop() {
        let mut state = SearchState::default();
        state.start_input();
        state.on_backspace(); // should not panic
        assert!(state.input_buffer.is_empty());
    }

    #[test]
    fn test_on_backspace_noop_when_not_input_mode() {
        let mut state = SearchState::default();
        state.input_buffer = "test".to_string();
        state.on_backspace();
        assert_eq!(state.input_buffer, "test");
    }

    #[test]
    fn test_confirm_with_input_transitions_to_active() {
        let mut state = SearchState::default();
        state.start_input();
        state.on_char('t');
        state.on_char('e');
        state.on_char('s');
        state.on_char('t');
        state.confirm();
        assert_eq!(state.mode, SearchMode::Active);
        assert_eq!(state.query, "test");
        assert!(state.input_buffer.is_empty());
    }

    #[test]
    fn test_confirm_empty_input_with_previous_query_reuses_query() {
        let mut state = SearchState::default();
        state.query = "previous".to_string();
        state.start_input();
        // Don't type anything; just press Enter.
        state.confirm();
        assert_eq!(state.mode, SearchMode::Active);
        assert_eq!(state.query, "previous");
    }

    #[test]
    fn test_confirm_empty_input_no_previous_query_goes_inactive() {
        let mut state = SearchState::default();
        state.start_input();
        state.confirm();
        assert_eq!(state.mode, SearchMode::Inactive);
    }

    #[test]
    fn test_confirm_noop_when_not_input_mode() {
        let mut state = SearchState::default();
        state.confirm(); // should not change state
        assert_eq!(state.mode, SearchMode::Inactive);
    }

    #[test]
    fn test_cancel_from_input_goes_inactive() {
        let mut state = SearchState::default();
        state.start_input();
        state.on_char('x');
        state.cancel();
        assert_eq!(state.mode, SearchMode::Inactive);
        assert!(state.input_buffer.is_empty());
    }

    #[test]
    fn test_cancel_from_active_clears_everything() {
        let mut state = SearchState::default();
        state.start_input();
        state.on_char('t');
        state.confirm();
        assert_eq!(state.mode, SearchMode::Active);

        state.cancel();
        assert_eq!(state.mode, SearchMode::Inactive);
        assert!(state.query.is_empty());
        assert!(state.matches.is_empty());
        assert!(state.current_match_index.is_none());
    }

    #[test]
    fn test_cancel_from_inactive_noop() {
        let mut state = SearchState::default();
        state.cancel();
        assert_eq!(state.mode, SearchMode::Inactive);
    }

    // -- Navigation tests ---------------------------------------------------

    #[test]
    fn test_next_match_advances() {
        let mut state = SearchState::default();
        state.matches = vec![
            SearchMatch {
                line_index: 0,
                byte_start: 0,
                byte_len: 3,
            },
            SearchMatch {
                line_index: 1,
                byte_start: 0,
                byte_len: 3,
            },
            SearchMatch {
                line_index: 2,
                byte_start: 0,
                byte_len: 3,
            },
        ];
        state.current_match_index = Some(0);
        state.next_match();
        assert_eq!(state.current_match_index, Some(1));
    }

    #[test]
    fn test_next_match_wraps() {
        let mut state = SearchState::default();
        state.matches = vec![
            SearchMatch {
                line_index: 0,
                byte_start: 0,
                byte_len: 3,
            },
            SearchMatch {
                line_index: 1,
                byte_start: 0,
                byte_len: 3,
            },
        ];
        state.current_match_index = Some(1);
        state.next_match();
        assert_eq!(state.current_match_index, Some(0));
    }

    #[test]
    fn test_next_match_from_none_goes_to_zero() {
        let mut state = SearchState::default();
        state.matches = vec![SearchMatch {
            line_index: 0,
            byte_start: 0,
            byte_len: 3,
        }];
        state.current_match_index = None;
        state.next_match();
        assert_eq!(state.current_match_index, Some(0));
    }

    #[test]
    fn test_next_match_empty_matches_noop() {
        let mut state = SearchState::default();
        state.next_match();
        assert!(state.current_match_index.is_none());
    }

    #[test]
    fn test_prev_match_goes_back() {
        let mut state = SearchState::default();
        state.matches = vec![
            SearchMatch {
                line_index: 0,
                byte_start: 0,
                byte_len: 3,
            },
            SearchMatch {
                line_index: 1,
                byte_start: 0,
                byte_len: 3,
            },
            SearchMatch {
                line_index: 2,
                byte_start: 0,
                byte_len: 3,
            },
        ];
        state.current_match_index = Some(2);
        state.prev_match();
        assert_eq!(state.current_match_index, Some(1));
    }

    #[test]
    fn test_prev_match_wraps() {
        let mut state = SearchState::default();
        state.matches = vec![
            SearchMatch {
                line_index: 0,
                byte_start: 0,
                byte_len: 3,
            },
            SearchMatch {
                line_index: 1,
                byte_start: 0,
                byte_len: 3,
            },
        ];
        state.current_match_index = Some(0);
        state.prev_match();
        assert_eq!(state.current_match_index, Some(1));
    }

    #[test]
    fn test_prev_match_from_none_goes_to_last() {
        let mut state = SearchState::default();
        state.matches = vec![
            SearchMatch {
                line_index: 0,
                byte_start: 0,
                byte_len: 3,
            },
            SearchMatch {
                line_index: 1,
                byte_start: 0,
                byte_len: 3,
            },
        ];
        state.current_match_index = None;
        state.prev_match();
        assert_eq!(state.current_match_index, Some(1));
    }

    #[test]
    fn test_prev_match_empty_matches_noop() {
        let mut state = SearchState::default();
        state.prev_match();
        assert!(state.current_match_index.is_none());
    }

    // -- Match counter display tests ----------------------------------------

    #[test]
    fn test_match_counter_display_active_with_matches() {
        let mut state = SearchState::default();
        state.mode = SearchMode::Active;
        state.matches = vec![
            SearchMatch {
                line_index: 0,
                byte_start: 0,
                byte_len: 3,
            },
            SearchMatch {
                line_index: 1,
                byte_start: 0,
                byte_len: 3,
            },
            SearchMatch {
                line_index: 2,
                byte_start: 0,
                byte_len: 3,
            },
        ];
        state.current_match_index = Some(2);
        assert_eq!(state.match_counter_display(), Some("[3/3]".to_string()));
    }

    #[test]
    fn test_match_counter_display_active_no_matches() {
        let mut state = SearchState::default();
        state.mode = SearchMode::Active;
        assert_eq!(state.match_counter_display(), Some("[0/0]".to_string()));
    }

    #[test]
    fn test_match_counter_display_active_no_current() {
        let mut state = SearchState::default();
        state.mode = SearchMode::Active;
        state.matches = vec![SearchMatch {
            line_index: 0,
            byte_start: 0,
            byte_len: 3,
        }];
        state.current_match_index = None;
        assert_eq!(state.match_counter_display(), Some("[0/1]".to_string()));
    }

    #[test]
    fn test_match_counter_display_inactive() {
        let state = SearchState::default();
        assert!(state.match_counter_display().is_none());
    }

    #[test]
    fn test_match_counter_display_input_mode() {
        let mut state = SearchState::default();
        state.mode = SearchMode::Input;
        assert!(state.match_counter_display().is_none());
    }

    // -- Helper method tests ------------------------------------------------

    #[test]
    fn test_is_active() {
        let mut state = SearchState::default();
        assert!(!state.is_active());
        state.mode = SearchMode::Active;
        assert!(state.is_active());
    }

    #[test]
    fn test_is_input() {
        let mut state = SearchState::default();
        assert!(!state.is_input());
        state.mode = SearchMode::Input;
        assert!(state.is_input());
    }

    #[test]
    fn test_current_match_line() {
        let mut state = SearchState::default();
        assert!(state.current_match_line().is_none());

        state.matches = vec![
            SearchMatch {
                line_index: 5,
                byte_start: 0,
                byte_len: 3,
            },
            SearchMatch {
                line_index: 10,
                byte_start: 0,
                byte_len: 3,
            },
        ];
        state.current_match_index = Some(1);
        assert_eq!(state.current_match_line(), Some(10));
    }

    #[test]
    fn test_current_match_line_no_index() {
        let mut state = SearchState::default();
        state.matches = vec![SearchMatch {
            line_index: 5,
            byte_start: 0,
            byte_len: 3,
        }];
        assert!(state.current_match_line().is_none());
    }

    // -- UTF-8 / multi-byte character tests --------------------------------

    #[test]
    fn test_find_matches_multibyte_ascii_after_emoji() {
        // "🎉hello" – emoji is 4 bytes, "hello" starts at byte 4.
        let text = "🎉hello";
        let matches = find_matches(text, "hello");
        assert_eq!(matches, vec![(4, 5)]);
        // Verify slicing is safe on the original text.
        let (start, len) = matches[0];
        assert_eq!(&text[start..start + len], "hello");
    }

    #[test]
    fn test_find_matches_multibyte_cjk_characters() {
        // "日本語test日本語" – each CJK char is 3 bytes.
        // "test" starts at byte 9 (3 chars * 3 bytes each).
        let text = "日本語test日本語";
        let matches = find_matches(text, "test");
        assert_eq!(matches, vec![(9, 4)]);
        let (start, len) = matches[0];
        assert_eq!(&text[start..start + len], "test");
    }

    #[test]
    fn test_find_matches_multibyte_search_for_cjk() {
        // Search for a multi-byte query within multi-byte text.
        let text = "こんにちは世界";
        let matches = find_matches(text, "世界");
        // "世界" starts at byte 15 (5 chars * 3 bytes each).
        assert_eq!(matches, vec![(15, 6)]);
        let (start, len) = matches[0];
        assert_eq!(&text[start..start + len], "世界");
    }

    #[test]
    fn test_find_matches_turkish_i_uppercase() {
        // Turkish İ (U+0130) is 2 bytes in UTF-8.
        // Its lowercase is "i\u{0307}" (i + combining dot above) = 3 bytes.
        // Searching for "i" in lowercase should match "İ" case-insensitively
        // only if the lowercased form contains "i".
        // İ.to_lowercase() = "i̇" = ['i', '\u{0307}'], so searching for "i̇"
        // should match and the returned offsets must be valid for the original.
        let text = "aİb";
        // "İ" lowercases to "i\u{0307}" (3 bytes in lowered text).
        // Search for the full lowercased form "i\u{0307}" to get a clean match.
        let matches = find_matches(text, "i\u{0307}");
        assert_eq!(matches.len(), 1);
        let (start, len) = matches[0];
        // The match should point to "İ" in the original text (byte 1, len 2).
        assert_eq!(start, 1);
        assert_eq!(len, 2);
        assert_eq!(&text[start..start + len], "İ");
    }

    #[test]
    fn test_find_matches_german_eszett_case_insensitive() {
        // Capital sharp S: ẞ (U+1E9E, 3 bytes) lowercases to "ß" (U+00DF, 2 bytes).
        // Searching for "ß" should find ẞ, and offsets must be valid for the original.
        let text = "aẞb";
        let matches = find_matches(text, "ß");
        assert_eq!(matches.len(), 1);
        let (start, len) = matches[0];
        // ẞ starts at byte 1 and is 3 bytes in the original.
        assert_eq!(start, 1);
        assert_eq!(len, 3);
        assert_eq!(&text[start..start + len], "ẞ");
    }

    #[test]
    fn test_find_matches_mixed_multibyte_multiple_matches() {
        // Text with mixed ASCII and multi-byte, multiple matches.
        let text = "café☕café";
        // "café" in original: c(1) + a(1) + f(1) + é(2) = 5 bytes.
        // "☕" is 3 bytes. Second "café" starts at byte 5+3=8.
        let matches = find_matches(text, "café");
        assert_eq!(matches.len(), 2);
        let (s1, l1) = matches[0];
        let (s2, l2) = matches[1];
        assert_eq!(&text[s1..s1 + l1], "café");
        assert_eq!(&text[s2..s2 + l2], "café");
        assert_eq!(s1, 0);
        assert_eq!(l1, 5); // c(1) + a(1) + f(1) + é(2)
        assert_eq!(s2, 8); // after first café(5) + ☕(3)
        assert_eq!(l2, 5);
    }

    #[test]
    fn test_find_matches_case_insensitive_with_accents() {
        // É (U+00C9, 2 bytes) lowercases to é (U+00E9, 2 bytes) – same byte length.
        let text = "rÉsumÉ";
        let matches = find_matches(text, "é");
        assert_eq!(matches.len(), 2);
        for (start, len) in &matches {
            // Each match should be 2 bytes (one É/é char) in the original.
            assert_eq!(*len, 2);
            // Verify slicing doesn't panic.
            let _slice = &text[*start..*start + *len];
        }
    }

    #[test]
    fn test_find_matches_offsets_are_valid_for_original_text() {
        // Comprehensive safety test: for any returned match, slicing the
        // original text must not panic.
        let test_cases = vec![
            ("hello world", "world"),
            ("🎉🎊celebration🎉", "celebration"),
            ("aİbİc", "İ"),
            ("日本語テスト", "テスト"),
            ("mixedミックス", "ミックス"),
            ("AAAAA", "aa"),
        ];
        for (text, query) in test_cases {
            let matches = find_matches(text, query);
            for (start, len) in &matches {
                // This must not panic on UTF-8 boundary.
                let _slice = &text[*start..*start + *len];
            }
        }
    }

    #[test]
    fn test_build_lower_to_orig_map_ascii() {
        let map = build_lower_to_orig_map("abc");
        // 'a','b','c' are each 1 byte, lowercase is same.
        assert_eq!(map, vec![(0, 0), (1, 1), (2, 2), (3, 3)]);
    }

    #[test]
    fn test_build_lower_to_orig_map_multibyte() {
        // "aİb": a=1byte, İ=2bytes (orig) -> i+combining_dot=3bytes (lower), b=1byte
        let map = build_lower_to_orig_map("aİb");
        assert_eq!(
            map,
            vec![
                (0, 0), // 'a': lower_offset=0, orig_offset=0
                (1, 1), // 'İ': lower_offset=1, orig_offset=1
                (4, 3), // 'b': lower_offset=1+3=4, orig_offset=1+2=3
                (5, 4), // sentinel: lower_offset=4+1=5, orig_offset=3+1=4
            ]
        );
    }
}
