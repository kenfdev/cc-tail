use std::collections::VecDeque;

use crate::log_entry::LogEntry;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Default byte budget: 50 MB.
///
/// This bounds the total estimated byte size of all entries held in the
/// ring buffer. When a push would exceed this budget, oldest entries are
/// evicted until there is room.
pub const DEFAULT_BYTE_BUDGET: usize = 50 * 1024 * 1024;

// ---------------------------------------------------------------------------
// Internal entry wrapper
// ---------------------------------------------------------------------------

/// A `LogEntry` together with its cached byte-size estimate.
///
/// The byte size is computed once at insertion time (via
/// [`LogEntry::estimated_byte_size`]) and stored alongside the entry so
/// that eviction accounting is O(1).
struct SizedEntry {
    entry: LogEntry,
    byte_size: usize,
}

// ---------------------------------------------------------------------------
// RingBuffer
// ---------------------------------------------------------------------------

/// A byte-budgeted ring buffer for [`LogEntry`] values.
///
/// Entries are stored in insertion order. When pushing a new entry would
/// cause `total_bytes` to exceed the configured `byte_budget`, the oldest
/// entries (front of the deque) are evicted one at a time until there is
/// room.
///
/// **Edge case:** if a single entry is larger than the entire budget, the
/// buffer is drained first and the oversized entry is accepted as the sole
/// occupant.
pub struct RingBuffer {
    entries: VecDeque<SizedEntry>,
    total_bytes: usize,
    byte_budget: usize,
}

impl RingBuffer {
    /// Create a new `RingBuffer` with the given byte budget.
    pub fn new(budget: usize) -> Self {
        Self {
            entries: VecDeque::new(),
            total_bytes: 0,
            byte_budget: budget,
        }
    }

    /// Create a new `RingBuffer` with the [`DEFAULT_BYTE_BUDGET`] (50 MB).
    pub fn with_default_budget() -> Self {
        Self::new(DEFAULT_BYTE_BUDGET)
    }

    /// Push an entry into the buffer, evicting oldest entries as needed.
    ///
    /// The entry's byte size is computed once via
    /// [`LogEntry::estimated_byte_size`] and cached for O(1) eviction
    /// accounting.
    pub fn push(&mut self, entry: LogEntry) {
        let byte_size = entry.estimated_byte_size();

        // Evict oldest entries while the new entry would exceed the budget.
        // If the single entry is larger than the budget, drain everything
        // and accept it as the sole occupant.
        while self.total_bytes + byte_size > self.byte_budget {
            match self.entries.pop_front() {
                Some(evicted) => {
                    self.total_bytes -= evicted.byte_size;
                }
                None => {
                    // Buffer is empty but entry still exceeds budget —
                    // accept the oversized entry.
                    break;
                }
            }
        }

        self.total_bytes += byte_size;
        self.entries.push_back(SizedEntry { entry, byte_size });
    }

    /// Iterate over all entries in insertion order (oldest first).
    pub fn iter(&self) -> impl Iterator<Item = &LogEntry> {
        self.entries.iter().map(|se| &se.entry)
    }

    /// Iterate over entries that satisfy `predicate`, in insertion order.
    pub fn iter_filtered<'a, F>(&'a self, predicate: F) -> impl Iterator<Item = &'a LogEntry>
    where
        F: Fn(&LogEntry) -> bool + 'a,
    {
        self.entries
            .iter()
            .map(|se| &se.entry)
            .filter(move |e| predicate(e))
    }

    /// Total estimated byte size of all entries currently in the buffer.
    #[allow(dead_code)]
    pub fn byte_size(&self) -> usize {
        self.total_bytes
    }

    /// Number of entries currently in the buffer.
    #[allow(dead_code)]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Returns `true` if the buffer contains no entries.
    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Remove all entries and reset the byte counter to zero.
    pub fn clear(&mut self) {
        self.entries.clear();
        self.total_bytes = 0;
    }

    /// The configured byte budget for this buffer.
    #[allow(dead_code)]
    pub fn byte_budget(&self) -> usize {
        self.byte_budget
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::log_entry::{parse_jsonl_line, EntryType};

    // -- Helpers ----------------------------------------------------------

    /// Build a minimal `LogEntry` of approximately the given byte size.
    ///
    /// The actual serialized size will be close to (but not necessarily
    /// exactly equal to) `target_bytes` because of JSON overhead. The
    /// returned entry carries a `session_id` padded to reach the target.
    fn make_entry_approx(target_bytes: usize) -> LogEntry {
        // Start with a minimal entry and measure its base size.
        let base = parse_jsonl_line(r#"{"type": "user"}"#).unwrap();
        let base_size = base.estimated_byte_size();

        // We pad the session_id to reach the target. Each character in the
        // session_id adds roughly one byte to the serialized JSON (plus
        // the key overhead which is already included after the first char).
        let padding = if target_bytes > base_size {
            target_bytes - base_size
        } else {
            0
        };

        let padded_id = "x".repeat(padding);
        let json = format!(r#"{{"type": "user", "sessionId": "{}"}}"#, padded_id);
        parse_jsonl_line(&json).unwrap()
    }

    /// Build a `LogEntry` with a specific entry type and session_id.
    fn make_entry_with_type(entry_type: &str, session_id: &str) -> LogEntry {
        let json = format!(
            r#"{{"type": "{}", "sessionId": "{}"}}"#,
            entry_type, session_id
        );
        parse_jsonl_line(&json).unwrap()
    }

    // -- 1. Basic push and iter -------------------------------------------

    #[test]
    fn test_basic_push_and_iter() {
        let mut buf = RingBuffer::new(10_000);

        let e1 = make_entry_with_type("user", "s1");
        let e2 = make_entry_with_type("assistant", "s2");
        buf.push(e1);
        buf.push(e2);

        let ids: Vec<_> = buf
            .iter()
            .map(|e| e.session_id.as_deref().unwrap())
            .collect();
        assert_eq!(ids, vec!["s1", "s2"]);
    }

    // -- 2. Byte size tracking --------------------------------------------

    #[test]
    fn test_byte_size_tracking() {
        let mut buf = RingBuffer::new(100_000);
        assert_eq!(buf.byte_size(), 0);

        let e1 = make_entry_with_type("user", "sess-a");
        let expected_size = e1.estimated_byte_size();
        buf.push(e1);
        assert_eq!(buf.byte_size(), expected_size);

        let e2 = make_entry_with_type("assistant", "sess-b");
        let expected_size2 = e2.estimated_byte_size();
        buf.push(e2);
        assert_eq!(buf.byte_size(), expected_size + expected_size2);
    }

    // -- 3. Eviction behavior (single eviction) ---------------------------

    #[test]
    fn test_eviction_removes_oldest() {
        // Create a buffer with a tight budget.
        let entry = make_entry_with_type("user", "measure");
        let entry_size = entry.estimated_byte_size();

        // Budget fits exactly 2 entries.
        let budget = entry_size * 2;
        let mut buf = RingBuffer::new(budget);

        let e1 = make_entry_with_type("user", "first");
        let e2 = make_entry_with_type("user", "second");
        let e3 = make_entry_with_type("user", "third");

        buf.push(e1);
        buf.push(e2);
        assert_eq!(buf.len(), 2);

        // Pushing a third should evict the first.
        buf.push(e3);
        assert_eq!(buf.len(), 2);

        let ids: Vec<_> = buf
            .iter()
            .map(|e| e.session_id.as_deref().unwrap())
            .collect();
        assert_eq!(ids, vec!["second", "third"]);
    }

    // -- 4. Multiple evictions per push -----------------------------------

    #[test]
    fn test_multiple_evictions_per_push() {
        // Use identically-sized entries so the math is exact.
        let e0 = make_entry_with_type("user", "s0");
        let e1 = make_entry_with_type("user", "s1");
        let e2 = make_entry_with_type("user", "s2");
        let e3 = make_entry_with_type("user", "s3");
        let small_size = e0.estimated_byte_size();

        // All four entries have the same serialized size.
        assert_eq!(e1.estimated_byte_size(), small_size);
        assert_eq!(e2.estimated_byte_size(), small_size);
        assert_eq!(e3.estimated_byte_size(), small_size);

        // Budget fits exactly 4 small entries.
        let budget = small_size * 4;
        let mut buf = RingBuffer::new(budget);

        buf.push(e0);
        buf.push(e1);
        buf.push(e2);
        buf.push(e3);
        assert_eq!(buf.len(), 4);

        // Build a large entry by iterating until we find one that
        // actually exceeds 2 small entries in serialized size.
        let mut padding_len = small_size * 2;
        let large = loop {
            let candidate = make_entry_approx(padding_len);
            if candidate.estimated_byte_size() > small_size * 2 {
                break candidate;
            }
            padding_len += 10;
        };
        let large_size = large.estimated_byte_size();

        buf.push(large);

        // The large entry (>2 small) plus 1 small would exceed 3 small,
        // so at least 3 of the 4 small entries should be evicted.
        // Remaining: at most 1 small + 1 large.
        assert!(buf.byte_size() <= budget);
        assert!(
            buf.len() <= 2,
            "expected at most 2 entries after eviction, got {} (large_size={}, small_size={})",
            buf.len(),
            large_size,
            small_size,
        );

        // Verify s0 and s1 were evicted (they are oldest).
        let ids: Vec<_> = buf.iter().filter_map(|e| e.session_id.as_deref()).collect();
        assert!(
            !ids.contains(&"s0"),
            "s0 should have been evicted, but found: {:?}",
            ids
        );
        assert!(
            !ids.contains(&"s1"),
            "s1 should have been evicted, but found: {:?}",
            ids
        );
    }

    // -- 5. Single oversized entry ----------------------------------------

    #[test]
    fn test_single_oversized_entry_accepted() {
        // Budget is tiny (10 bytes); push an entry much larger.
        let mut buf = RingBuffer::new(10);

        let big = make_entry_with_type("user", "oversized-session-id");
        let big_size = big.estimated_byte_size();
        assert!(
            big_size > 10,
            "entry must be larger than budget for this test"
        );

        buf.push(big);

        assert_eq!(buf.len(), 1);
        assert_eq!(buf.byte_size(), big_size);
        assert_eq!(
            buf.iter().next().unwrap().session_id.as_deref(),
            Some("oversized-session-id")
        );
    }

    // -- 6. Filtered iteration --------------------------------------------

    #[test]
    fn test_iter_filtered() {
        let mut buf = RingBuffer::new(100_000);

        buf.push(make_entry_with_type("user", "u1"));
        buf.push(make_entry_with_type("assistant", "a1"));
        buf.push(make_entry_with_type("user", "u2"));
        buf.push(make_entry_with_type("assistant", "a2"));

        let user_ids: Vec<_> = buf
            .iter_filtered(|e| e.entry_type == EntryType::User)
            .map(|e| e.session_id.as_deref().unwrap())
            .collect();
        assert_eq!(user_ids, vec!["u1", "u2"]);

        let assistant_ids: Vec<_> = buf
            .iter_filtered(|e| e.entry_type == EntryType::Assistant)
            .map(|e| e.session_id.as_deref().unwrap())
            .collect();
        assert_eq!(assistant_ids, vec!["a1", "a2"]);
    }

    // -- 7. Clear ---------------------------------------------------------

    #[test]
    fn test_clear() {
        let mut buf = RingBuffer::new(100_000);

        buf.push(make_entry_with_type("user", "s1"));
        buf.push(make_entry_with_type("user", "s2"));
        assert!(!buf.is_empty());

        buf.clear();

        assert!(buf.is_empty());
        assert_eq!(buf.len(), 0);
        assert_eq!(buf.byte_size(), 0);
    }

    // -- 8. Empty buffer --------------------------------------------------

    #[test]
    fn test_empty_buffer() {
        let buf = RingBuffer::new(1_000);

        assert!(buf.is_empty());
        assert_eq!(buf.len(), 0);
        assert_eq!(buf.byte_size(), 0);
        assert_eq!(buf.iter().count(), 0);
    }

    // -- 9. len and is_empty ---------------------------------------------

    #[test]
    fn test_len_and_is_empty() {
        let mut buf = RingBuffer::new(100_000);
        assert!(buf.is_empty());
        assert_eq!(buf.len(), 0);

        buf.push(make_entry_with_type("user", "s1"));
        assert!(!buf.is_empty());
        assert_eq!(buf.len(), 1);

        buf.push(make_entry_with_type("user", "s2"));
        assert_eq!(buf.len(), 2);

        buf.clear();
        assert!(buf.is_empty());
        assert_eq!(buf.len(), 0);
    }

    // -- 10. Default budget -----------------------------------------------

    #[test]
    fn test_default_budget() {
        let buf = RingBuffer::with_default_budget();
        assert_eq!(buf.byte_budget(), DEFAULT_BYTE_BUDGET);
        assert_eq!(buf.byte_budget(), 50 * 1024 * 1024);
    }

    // -- 11. Mixed entry sizes --------------------------------------------

    #[test]
    fn test_mixed_entry_sizes() {
        let small = make_entry_with_type("user", "s");
        let small_size = small.estimated_byte_size();

        // Budget fits ~5 small entries.
        let budget = small_size * 5;
        let mut buf = RingBuffer::new(budget);

        // Push 3 small entries.
        buf.push(make_entry_with_type("user", "a"));
        buf.push(make_entry_with_type("user", "b"));
        buf.push(make_entry_with_type("user", "c"));

        // Push a medium entry (~2x small).
        let medium = make_entry_approx(small_size * 2);
        buf.push(medium);

        // Should still fit: 3 small + 1 medium = 5 units.
        assert_eq!(buf.len(), 4);

        // Push another medium — should evict smallest entries from front.
        let medium2 = make_entry_approx(small_size * 2);
        buf.push(medium2);

        // After eviction, buffer should have evicted enough to fit.
        assert!(buf.byte_size() <= budget);
        assert!(buf.len() >= 2); // at least the two medium entries
    }

    // -- 12. Stress test: 10k entries with tight budget -------------------

    #[test]
    fn test_stress_10k_entries_tight_budget_byte_accounting() {
        // Use a tight budget that can only hold a fraction of 10,000 entries.
        // After all pushes, verify:
        //   1. byte_size() equals sum of remaining entries' estimated_byte_size()
        //   2. byte_size() <= budget
        let entry_sample = make_entry_with_type("user", "measure");
        let entry_approx_size = entry_sample.estimated_byte_size();

        // Budget fits roughly 50 entries (intentionally tight for 10,000 pushes).
        let budget = entry_approx_size * 50;
        let mut buf = RingBuffer::new(budget);

        for i in 0..10_000 {
            let entry = make_entry_with_type("user", &format!("s{}", i));
            buf.push(entry);
        }

        // Invariant 1: byte_size() must not exceed the budget.
        assert!(
            buf.byte_size() <= budget,
            "byte_size() {} exceeds budget {}",
            buf.byte_size(),
            budget,
        );

        // Invariant 2: byte_size() must equal the sum of remaining entries'
        // estimated_byte_size() values.
        let sum_of_entries: usize = buf.iter().map(|e| e.estimated_byte_size()).sum();
        assert_eq!(
            buf.byte_size(),
            sum_of_entries,
            "byte_size() {} != sum of remaining entries {} (len={})",
            buf.byte_size(),
            sum_of_entries,
            buf.len(),
        );

        // Sanity: buffer should contain a reasonable number of entries.
        assert!(
            buf.len() > 0 && buf.len() <= 50,
            "expected 1-50 entries, got {}",
            buf.len(),
        );
    }

    // -- 13. Budget boundary pushes (exact fit) ---------------------------

    #[test]
    fn test_budget_boundary_exact_fit() {
        let entry = make_entry_with_type("user", "measure");
        let entry_size = entry.estimated_byte_size();

        // Budget fits exactly 3 entries.
        let budget = entry_size * 3;
        let mut buf = RingBuffer::new(budget);

        // Push exactly 3 entries — should all fit.
        buf.push(make_entry_with_type("user", "one"));
        buf.push(make_entry_with_type("user", "two"));
        buf.push(make_entry_with_type("user", "three"));
        assert_eq!(buf.len(), 3);
        assert!(buf.byte_size() <= budget);

        // Push one more — should evict exactly one.
        buf.push(make_entry_with_type("user", "four"));
        assert_eq!(buf.len(), 3);

        let ids: Vec<_> = buf
            .iter()
            .map(|e| e.session_id.as_deref().unwrap())
            .collect();
        assert_eq!(ids, vec!["two", "three", "four"]);
    }
}
