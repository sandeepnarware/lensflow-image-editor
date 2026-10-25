//! Comparing release versions (for "a new version is available").

/// `latest` is a strictly newer semver release than `current`. A leading `v` is accepted;
/// a string that is not semver is never newer.
pub fn newer(latest: &str, current: &str) -> bool {
    match (parse(latest), parse(current)) {
        (Some(latest), Some(current)) => latest > current,
        _ => false,
    }
}

fn parse(text: &str) -> Option<semver::Version> {
    let text = text.trim();
    semver::Version::parse(text.strip_prefix('v').unwrap_or(text)).ok()
}

#[cfg(test)]
mod tests {
    use super::newer;

    #[test]
    fn newer_compares_semver() {
        assert!(newer("1.2.0", "1.1.9"));
        assert!(newer("1.10.0", "1.9.0"));
        assert!(newer("v1.0.1", "1.0.0"));
        assert!(!newer("1.1.9", "1.2.0"));
        assert!(!newer("garbage", "1.0.0"));
        assert!(!newer("1.0.0", "garbage"));
        assert!(!newer("1.0.0", "1.0.0"));
        assert!(!newer("v1.0.0", "1.0.0"));
    }
}
