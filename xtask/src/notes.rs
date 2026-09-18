//! The version a tag names, and the release notes for it.
//!
//! A release is cut by pushing a tag that is the bare version, `0.1.0` or `0.2.0-rc.1`, with no
//! `v` in front (the user's call, 2026-09-18). The tag must name exactly the version the server
//! reports (`GET /api/v1/health`, `--version`), because the updater compares the two: a tag ahead
//! of the binary it carries would offer the same update again after every restart. The updater
//! itself still reads a `v`-prefixed tag, so a release tagged either way is found.
//!
//! The notes are the version's section of `CHANGELOG.md`, kept by hand (Keep a Changelog). A
//! release with no notes is refused rather than published empty.

use semver::Version;

/// What `check-version` found, printed as `key=value` lines a workflow appends to its outputs.
#[derive(Debug, PartialEq, Eq)]
pub struct Report {
    pub version: String,
    pub tag: String,
    pub prerelease: bool,
}

impl Report {
    pub fn lines(&self) -> String {
        format!("version={}\ntag={}\nprerelease={}\n", self.version, self.tag, self.prerelease)
    }
}

/// The tag must be the server's version, exactly, with no `v`. With no tag (a dry run from a
/// branch), the report is for the version itself, as the tag for it would be.
pub fn check_version(tag: Option<&str>, version: &str) -> Result<Report, String> {
    let parsed = Version::parse(version).map_err(|e| format!("the server's version {version:?} is not semver: {e}"))?;
    if let Some(tag) = tag {
        if tag.starts_with(['v', 'V']) && &tag[1..] == version {
            return Err(format!(
                "the tag is {tag}, but release tags are the bare version with no v in front, so it must be {version}; \
                 delete {tag} and tag the same commit {version}"
            ));
        }
        if tag != version {
            return Err(format!(
                "the tag is {tag}, but crates/gazelle-audio-server/Cargo.toml says {version}, so the tag must be {version}; \
                 bump the version and tag that commit, or delete the tag"
            ));
        }
    }
    Ok(Report { version: version.to_string(), tag: version.to_string(), prerelease: !parsed.pre.is_empty() })
}

/// The body of the `## [<version>]` section: everything up to the next `## ` heading or the link
/// references at the foot of the file, with blank lines trimmed from both ends. Whatever follows
/// the bracket on the heading line (a date) is not part of it.
///
/// Missing, doubled, or empty (nothing but blank lines and subheadings) are all errors.
pub fn section(changelog: &str, version: &str) -> Result<String, String> {
    let heading = format!("## [{version}]");
    let is_heading = |line: &str| {
        let line = line.trim_end();
        line == heading || line.strip_prefix(heading.as_str()).is_some_and(|rest| rest.starts_with(' '))
    };
    let lines: Vec<&str> = changelog.lines().collect();
    let starts: Vec<usize> = lines.iter().enumerate().filter(|(_, l)| is_heading(l)).map(|(i, _)| i).collect();
    let start = match starts.as_slice() {
        [] => return Err(format!("has no \"{heading}\" section; add the release notes for {version} before tagging")),
        [one] => *one,
        _ => return Err(format!("has {} \"{heading}\" sections; keep one", starts.len())),
    };

    let body: Vec<&str> = lines[start + 1..]
        .iter()
        .take_while(|line| !line.starts_with("## ") && !is_link_reference(line))
        .copied()
        .collect();
    let first = body.iter().position(|l| !l.trim().is_empty());
    let last = body.iter().rposition(|l| !l.trim().is_empty());
    let body = match (first, last) {
        (Some(first), Some(last)) => &body[first..=last],
        _ => &[][..],
    };
    if !body.iter().any(|l| !l.trim().is_empty() && !l.trim_start().starts_with('#')) {
        return Err(format!("the \"{heading}\" section is empty; write what changed for users before tagging"));
    }
    Ok(body.iter().map(|l| format!("{l}\n")).collect())
}

/// `[0.1.0]: https://...`, the link definitions Keep a Changelog puts at the foot of the file.
fn is_link_reference(line: &str) -> bool {
    line.starts_with('[') && line.contains("]: ")
}

#[cfg(test)]
mod tests {
    use super::*;

    const CHANGELOG: &str = "\
# Changelog

Intro text.

## [Unreleased]

### Added

- Something not yet released.

## [0.2.0-rc.1] - 2026-10-01

### Fixed

- A fix.

## [0.1.0]

The first release.

### Added

- The mixer.
- The router.

[Unreleased]: https://github.com/doesdev/gazelle-audio/compare/0.1.0...HEAD
[0.1.0]: https://github.com/doesdev/gazelle-audio/releases/tag/0.1.0
";

    #[test]
    fn a_tag_must_be_the_bare_version_exactly() {
        assert_eq!(
            check_version(Some("0.1.0"), "0.1.0"),
            Ok(Report { version: "0.1.0".into(), tag: "0.1.0".into(), prerelease: false })
        );
        for wrong in ["0.1.1", "0.1", "0.1.0-rc.1", "0.1.0 ", "release-0.1.0"] {
            let error = check_version(Some(wrong), "0.1.0").unwrap_err();
            assert!(error.contains("so the tag must be 0.1.0"), "{wrong}: {error}");
        }
    }

    #[test]
    fn a_v_prefixed_tag_is_refused_with_the_reason() {
        for prefixed in ["v0.1.0", "V0.1.0"] {
            let error = check_version(Some(prefixed), "0.1.0").unwrap_err();
            assert!(error.contains("bare version with no v in front"), "{prefixed}: {error}");
        }
        // A v in front of some other version is still, first of all, the wrong version.
        assert!(check_version(Some("v0.1.1"), "0.1.0").unwrap_err().contains("must be 0.1.0"));
    }

    #[test]
    fn a_semver_pre_release_is_a_prerelease_and_build_metadata_is_not() {
        assert!(check_version(Some("0.2.0-rc.1"), "0.2.0-rc.1").unwrap().prerelease);
        assert!(!check_version(Some("0.2.0+build.5"), "0.2.0+build.5").unwrap().prerelease);
        assert_eq!(check_version(Some("0.2.0-rc.1"), "0.2.0-rc.1").unwrap().lines(), "version=0.2.0-rc.1\ntag=0.2.0-rc.1\nprerelease=true\n");
    }

    #[test]
    fn with_no_tag_it_reports_the_version_as_its_tag_would() {
        assert_eq!(check_version(None, "0.3.0").unwrap().tag, "0.3.0");
        assert!(check_version(None, "not-a-version").unwrap_err().contains("not semver"));
    }

    #[test]
    fn the_section_is_the_body_under_the_versions_heading_and_nothing_else() {
        assert_eq!(section(CHANGELOG, "0.1.0").unwrap(), "The first release.\n\n### Added\n\n- The mixer.\n- The router.\n");
        // A date after the heading is allowed and not part of the notes.
        assert_eq!(section(CHANGELOG, "0.2.0-rc.1").unwrap(), "### Fixed\n\n- A fix.\n");
    }

    #[test]
    fn a_version_is_matched_whole_not_by_prefix() {
        let text = "## [0.1.0-rc.1]\n\n- rc\n\n## [0.1.00]\n\n- no\n";
        assert!(section(text, "0.1.0").unwrap_err().contains("has no"));
        assert_eq!(section(text, "0.1.0-rc.1").unwrap(), "- rc\n");
    }

    #[test]
    fn a_missing_empty_or_doubled_section_is_an_error() {
        assert!(section(CHANGELOG, "9.9.9").unwrap_err().contains("has no \"## [9.9.9]\" section"));
        let empty = "## [0.3.0]\n\n### Added\n\n## [0.2.0]\n\n- x\n";
        assert!(section(empty, "0.3.0").unwrap_err().contains("is empty"));
        assert!(section("## [0.3.0]\n", "0.3.0").unwrap_err().contains("is empty"));
        let doubled = "## [0.3.0]\n\n- a\n\n## [0.3.0]\n\n- b\n";
        assert!(section(doubled, "0.3.0").unwrap_err().contains("2 \"## [0.3.0]\" sections"));
    }

    #[test]
    fn the_repositorys_changelog_has_a_section_for_the_current_version() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../CHANGELOG.md");
        let text = std::fs::read_to_string(&path).expect("CHANGELOG.md at the repository root");
        assert!(text.contains("## [Unreleased]"), "CHANGELOG.md keeps an Unreleased section for the next release");
        section(&text, gazelle_audio_server::VERSION).expect("the version being built has release notes");
    }
}
