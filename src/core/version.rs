//! Version comparison for the update check (GitHub release tags like
//! `v0.2.0` vs `CARGO_PKG_VERSION`).

/// `1.2.3` / `v1.2.3` → numbers. Pre-release/build suffixes (`-beta.1`,
/// `+x`) are ignored; missing parts count as 0. None if not numeric.
fn parse(version: &str) -> Option<Vec<u64>> {
    let v = version.trim().trim_start_matches(['v', 'V']);
    let core = v.split(['-', '+']).next()?;
    core.split('.').map(|p| p.parse().ok()).collect()
}

/// Whether `latest` is a newer version than `current`. False when either
/// can't be parsed (never nag about an odd tag).
pub fn is_newer(latest: &str, current: &str) -> bool {
    let (Some(mut a), Some(mut b)) = (parse(latest), parse(current)) else { return false };
    let len = a.len().max(b.len());
    a.resize(len, 0);
    b.resize(len, 0);
    a > b
}

#[cfg(test)]
mod tests {
    use super::is_newer;

    #[test]
    fn compares_numerically() {
        assert!(is_newer("v0.2.0", "0.1.0"));
        assert!(is_newer("0.10.0", "0.9.9"));
        assert!(is_newer("v1", "0.9.0"));
        assert!(!is_newer("v0.1.0", "0.1.0"));
        assert!(!is_newer("0.1", "0.1.0"));
        assert!(!is_newer("v0.1.0", "0.2.0"));
    }

    #[test]
    fn ignores_suffixes_and_junk() {
        assert!(is_newer("v0.2.0-beta.1", "0.1.0"));
        assert!(!is_newer("nightly", "0.1.0"));
        assert!(!is_newer("", "0.1.0"));
    }
}
