//! A tiny, dependency-free glob matcher for the source scanner's
//! `include`/`exclude` glob matching for `[package]` scan filters.
//!
//! cbld ships without a glob crate — same "write the small thing ourselves"
//! stance as `json.rs` and `hash.rs`. The supported syntax is the familiar
//! path-glob subset:
//!
//! - `?` matches any single character within a path segment.
//! - `*` matches any run of characters (including none) within one segment; it
//!   never crosses a `/`.
//! - `**` matches zero or more whole path segments, so `tests/**` matches
//!   `tests` itself and everything under it, and `**/*_test.c` matches a
//!   `*_test.c` file at any depth.
//!
//! Patterns and paths are matched with `/` as the separator; callers pass
//! source-dir-relative paths with forward slashes.

/// True when `path` (a `/`-separated, source-dir-relative path) matches the
/// glob `pattern`.
pub fn matches(pattern: &str, path: &str) -> bool {
    let pat: Vec<&str> = pattern.split('/').filter(|s| !s.is_empty()).collect();
    let seg: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    match_segments(&pat, &seg)
}

/// True when `path` matches any pattern in `patterns`.
pub fn matches_any(patterns: &[String], path: &str) -> bool {
    patterns.iter().any(|p| matches(p, path))
}

fn match_segments(pat: &[&str], seg: &[&str]) -> bool {
    if pat.is_empty() {
        return seg.is_empty();
    }
    if pat[0] == "**" {
        // `**` consumes zero or more whole segments; try every split.
        for i in 0..=seg.len() {
            if match_segments(&pat[1..], &seg[i..]) {
                return true;
            }
        }
        return false;
    }
    if seg.is_empty() {
        return false;
    }
    if match_one(pat[0], seg[0]) {
        return match_segments(&pat[1..], &seg[1..]);
    }
    false
}

/// Match a single path segment against a single pattern segment, honoring the
/// intra-segment wildcards `*` and `?` (neither of which crosses `/`, since a
/// segment never contains one).
fn match_one(pat: &str, seg: &str) -> bool {
    let p: Vec<char> = pat.chars().collect();
    let s: Vec<char> = seg.chars().collect();
    wildcard(&p, &s)
}

fn wildcard(p: &[char], s: &[char]) -> bool {
    if p.is_empty() {
        return s.is_empty();
    }
    match p[0] {
        '*' => {
            // Zero-width match, or consume one char of `s` and retry.
            wildcard(&p[1..], s) || (!s.is_empty() && wildcard(p, &s[1..]))
        }
        '?' => !s.is_empty() && wildcard(&p[1..], &s[1..]),
        c => !s.is_empty() && s[0] == c && wildcard(&p[1..], &s[1..]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn star_stays_within_a_segment() {
        assert!(matches("*.c", "foo.c"));
        assert!(!matches("*.c", "sub/foo.c")); // `*` never crosses `/`
        assert!(matches("src/*.cpp", "src/main.cpp"));
        assert!(!matches("src/*.cpp", "src/deep/main.cpp"));
    }

    #[test]
    fn question_matches_exactly_one_char() {
        assert!(matches("v?.c", "v1.c"));
        assert!(!matches("v?.c", "v10.c"));
    }

    #[test]
    fn double_star_spans_zero_or_more_segments() {
        // `tests/**` matches the directory itself and anything under it —
        // exactly what pruning a test tree needs.
        assert!(matches("tests/**", "tests"));
        assert!(matches("tests/**", "tests/a.c"));
        assert!(matches("tests/**", "tests/unit/deep/b.c"));
        assert!(!matches("tests/**", "src/a.c"));
    }

    #[test]
    fn leading_double_star_matches_at_any_depth() {
        assert!(matches("**/*_test.c", "a_test.c"));
        assert!(matches("**/*_test.c", "src/util/parser_test.c"));
        assert!(!matches("**/*_test.c", "src/parser.c"));
    }

    #[test]
    fn double_star_in_the_middle() {
        assert!(matches("src/**/gen.cc", "src/gen.cc"));
        assert!(matches("src/**/gen.cc", "src/a/b/gen.cc"));
        assert!(!matches("src/**/gen.cc", "other/gen.cc"));
    }

    #[test]
    fn matches_any_is_an_or_over_patterns() {
        let pats = vec!["tests/**".to_string(), "**/*.pb.cc".to_string()];
        assert!(matches_any(&pats, "tests/x.c"));
        assert!(matches_any(&pats, "proto/msg.pb.cc"));
        assert!(!matches_any(&pats, "src/lib.cpp"));
        assert!(!matches_any(&[], "anything")); // empty set matches nothing
    }
}
