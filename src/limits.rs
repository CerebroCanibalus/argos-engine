//! Token budgets - payload size limits are a first-class feature.

/// Maximum characters for a result title (plus `...` when cut).
pub const TITLE_MAX_CHARS: usize = 200;

/// Maximum characters for a result snippet content (plus `...` when cut).
///
/// Note: `truncate_string` treats this as the content budget - the ellipsis
/// Flojo appends on cut is extra, so the true cap is `SNIPPET_MAX_CHARS + 3`.
pub const SNIPPET_MAX_CHARS: usize = 300;

/// Maximum characters for the compact source (hostname) of a result.
pub const SOURCE_MAX_CHARS: usize = 32;

/// Default number of results per call.
pub const DEFAULT_LIMIT: usize = 10;

/// Hard cap for results per call.
pub const MAX_LIMIT: usize = 50;

/// Clamp a requested limit into `[1, MAX_LIMIT]` (default [`DEFAULT_LIMIT`]).
pub fn clamp_limit(limit: Option<usize>) -> usize {
    limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT)
}

/// Clamp a requested page number into `>= 1`.
pub fn clamp_page(page: Option<usize>) -> usize {
    page.unwrap_or(1).max(1)
}

/// Truncate a compact source (hostname) to the [`SOURCE_MAX_CHARS`] budget.
pub fn truncate_source(s: &str) -> String {
    flojo_mcp::truncate_string(s, SOURCE_MAX_CHARS).0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limit_is_clamped() {
        assert_eq!(clamp_limit(None), DEFAULT_LIMIT);
        assert_eq!(clamp_limit(Some(0)), 1);
        assert_eq!(clamp_limit(Some(9999)), MAX_LIMIT);
    }

    #[test]
    fn page_never_below_one() {
        assert_eq!(clamp_page(None), 1);
        assert_eq!(clamp_page(Some(0)), 1);
    }
}
