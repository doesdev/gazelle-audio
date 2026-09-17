//! What a release is, how one is read out of the release source's answer, and which one — if
//! any — is newer than what is running.
//!
//! Nothing here touches the network or the disk, so every rule below is a unit test.

use std::collections::BTreeMap;

use semver::Version;

use super::settings::Channel;

/// The file listing every asset's SHA-256, and the detached ed25519 signature over it.
pub const SUMS_NAME: &str = "SHA256SUMS";
pub const SIGNATURE_NAME: &str = "SHA256SUMS.sig";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Asset {
    pub name: String,
    pub url: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Release {
    pub tag: String,
    pub version: Version,
    /// The source's own flag. A tag carrying a semver pre-release counts as one too, so a
    /// mis-flagged `v0.3.0-rc.1` still stays off the stable channel.
    pub prerelease: bool,
    /// Where a human can read what changed.
    pub page: String,
    pub assets: Vec<Asset>,
}

impl Release {
    pub fn is_prerelease(&self) -> bool {
        self.prerelease || !self.version.pre.is_empty()
    }

    pub fn asset(&self, name: &str) -> Option<&Asset> {
        self.assets.iter().find(|a| a.name == name)
    }
}

/// The asset holding the binary for one platform: the executable itself, not an archive, so
/// applying an update is a rename and never an unpack. See `update/mod.rs`.
pub fn binary_asset_name(stem: &str, target: &str) -> String {
    let suffix = if target.contains("windows") { ".exe" } else { "" };
    format!("{stem}-{target}{suffix}")
}

/// Read the release source's answer. Entries that do not parse — a tag that is not a version, a
/// missing asset list — are skipped rather than failing the whole check, so one bad release
/// cannot stop the app updating.
pub fn parse_releases(json: &serde_json::Value) -> Vec<Release> {
    let Some(entries) = json.as_array() else { return Vec::new() };
    entries
        .iter()
        .filter_map(|entry| {
            let tag = entry.get("tag_name")?.as_str()?.to_string();
            let version = Version::parse(tag.strip_prefix('v').unwrap_or(&tag)).ok()?;
            let assets = entry
                .get("assets")
                .and_then(|a| a.as_array())
                .map(|assets| {
                    assets
                        .iter()
                        .filter_map(|a| {
                            Some(Asset {
                                name: a.get("name")?.as_str()?.to_string(),
                                url: a.get("browser_download_url")?.as_str()?.to_string(),
                            })
                        })
                        .collect()
                })
                .unwrap_or_default();
            Some(Release {
                tag,
                version,
                prerelease: entry.get("prerelease").and_then(serde_json::Value::as_bool).unwrap_or(false),
                page: entry.get("html_url").and_then(serde_json::Value::as_str).unwrap_or_default().to_string(),
                assets,
            })
        })
        .collect()
}

/// The newest release worth offering: newer than what is running, on the chosen channel, and
/// carrying an asset for this platform. `None` means there is nothing to offer.
pub fn newest<'a>(releases: &'a [Release], channel: Channel, current: &Version, asset: &str) -> Option<&'a Release> {
    releases
        .iter()
        .filter(|r| channel == Channel::Prerelease || !r.is_prerelease())
        .filter(|r| r.version > *current)
        .filter(|r| r.asset(asset).is_some() && r.asset(SUMS_NAME).is_some() && r.asset(SIGNATURE_NAME).is_some())
        .max_by(|a, b| a.version.cmp(&b.version))
}

/// Parse a `SHA256SUMS` file: `<64 hex digits><spaces><name>` per line, as `sha256sum` writes
/// it. A line that does not parse is skipped; a name appearing twice keeps the first, so an
/// appended line cannot quietly redefine a file's hash.
pub fn parse_sums(text: &str) -> BTreeMap<String, [u8; 32]> {
    let mut sums = BTreeMap::new();
    for line in text.lines() {
        let line = line.trim();
        let Some((hex, name)) = line.split_once(char::is_whitespace) else { continue };
        // `sha256sum` marks binary mode with a leading `*` on the name.
        let name = name.trim_start().trim_start_matches('*').trim();
        let (Some(digest), false) = (from_hex(hex), name.is_empty()) else { continue };
        sums.entry(name.to_string()).or_insert(digest);
    }
    sums
}

/// 64 hex digits to 32 bytes.
pub fn from_hex(hex: &str) -> Option<[u8; 32]> {
    let bytes = hex.as_bytes();
    if bytes.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, pair) in bytes.chunks_exact(2).enumerate() {
        out[i] = u8::from_str_radix(std::str::from_utf8(pair).ok()?, 16).ok()?;
    }
    Some(out)
}

pub fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn release(tag: &str, prerelease: bool, assets: &[&str]) -> Release {
        Release {
            tag: tag.into(),
            version: Version::parse(tag.strip_prefix('v').unwrap_or(tag)).unwrap(),
            prerelease,
            page: format!("https://example.invalid/{tag}"),
            assets: assets.iter().map(|n| Asset { name: (*n).into(), url: format!("https://example.invalid/{tag}/{n}") }).collect(),
        }
    }

    const BIN: &str = "gazelle-audio-server-x86_64-pc-windows-msvc.exe";

    fn full(tag: &str, prerelease: bool) -> Release {
        release(tag, prerelease, &[BIN, SUMS_NAME, SIGNATURE_NAME])
    }

    #[test]
    fn an_asset_is_named_for_the_platform_it_was_built_for() {
        assert_eq!(binary_asset_name("gazelle-audio-server", "x86_64-pc-windows-msvc"), BIN);
        assert_eq!(binary_asset_name("gazelle-audio-serverw", "x86_64-pc-windows-msvc"), "gazelle-audio-serverw-x86_64-pc-windows-msvc.exe");
        assert_eq!(binary_asset_name("gazelle-audio-server", "aarch64-apple-darwin"), "gazelle-audio-server-aarch64-apple-darwin");
        assert_eq!(binary_asset_name("gazelle-audio-server", "x86_64-unknown-linux-gnu"), "gazelle-audio-server-x86_64-unknown-linux-gnu");
    }

    #[test]
    fn the_newest_newer_release_is_the_one_offered() {
        let releases = [full("v0.1.0", false), full("v0.3.1", false), full("v0.2.0", false)];
        let current = Version::parse("0.1.0").unwrap();
        assert_eq!(newest(&releases, Channel::Stable, &current, BIN).map(|r| r.tag.as_str()), Some("v0.3.1"));
    }

    #[test]
    fn the_running_version_and_anything_older_is_nothing_to_offer() {
        let releases = [full("v0.1.0", false), full("v0.0.9", false)];
        let current = Version::parse("0.1.0").unwrap();
        assert_eq!(newest(&releases, Channel::Stable, &current, BIN), None);
        assert_eq!(newest(&[], Channel::Stable, &current, BIN), None);
    }

    #[test]
    fn a_pre_release_is_ignored_unless_the_channel_asks_for_one() {
        let releases = [full("v0.2.0", false), full("v0.3.0-rc.1", true)];
        let current = Version::parse("0.1.0").unwrap();
        assert_eq!(newest(&releases, Channel::Stable, &current, BIN).map(|r| r.tag.as_str()), Some("v0.2.0"));
        assert_eq!(newest(&releases, Channel::Prerelease, &current, BIN).map(|r| r.tag.as_str()), Some("v0.3.0-rc.1"));
    }

    #[test]
    fn a_pre_release_tag_is_one_even_when_the_source_does_not_say_so() {
        let mislabelled = full("v0.3.0-rc.1", false);
        assert!(mislabelled.is_prerelease());
        let current = Version::parse("0.1.0").unwrap();
        assert_eq!(newest(&[mislabelled], Channel::Stable, &current, BIN), None);
    }

    #[test]
    fn a_release_without_a_binary_sums_or_signature_for_this_platform_is_not_offered() {
        let current = Version::parse("0.1.0").unwrap();
        for assets in [vec![SUMS_NAME, SIGNATURE_NAME], vec![BIN, SIGNATURE_NAME], vec![BIN, SUMS_NAME]] {
            assert_eq!(newest(&[release("v0.2.0", false, &assets)], Channel::Stable, &current, BIN), None, "{assets:?}");
        }
        let other_platform = release("v0.2.0", false, &["gazelle-audio-server-aarch64-apple-darwin", SUMS_NAME, SIGNATURE_NAME]);
        assert_eq!(newest(&[other_platform], Channel::Stable, &current, BIN), None);
    }

    #[test]
    fn the_sources_answer_reads_as_releases_and_a_bad_entry_is_skipped() {
        let json = serde_json::json!([
            {"tag_name": "v0.2.0", "prerelease": false, "html_url": "https://example.invalid/r/0.2.0",
             "assets": [{"name": BIN, "browser_download_url": "https://example.invalid/a/bin"}]},
            {"tag_name": "nightly", "prerelease": true, "assets": []},
            {"prerelease": false, "assets": []},
            {"tag_name": "v0.3.0-rc.1", "prerelease": true}
        ]);
        let releases = parse_releases(&json);
        assert_eq!(releases.len(), 2, "{releases:?}");
        assert_eq!(releases[0].version, Version::parse("0.2.0").unwrap());
        assert_eq!(releases[0].page, "https://example.invalid/r/0.2.0");
        assert_eq!(releases[0].asset(BIN).map(|a| a.url.as_str()), Some("https://example.invalid/a/bin"));
        assert_eq!(releases[0].asset("nothing"), None);
        assert_eq!(releases[1].tag, "v0.3.0-rc.1");
        assert!(releases[1].assets.is_empty());
        assert_eq!(parse_releases(&serde_json::json!({"message": "Not Found"})), Vec::new());
    }

    #[test]
    fn a_sums_file_reads_as_a_name_to_digest_map() {
        let a = "0".repeat(64);
        let b = format!("{}{}", "ab".repeat(31), "cd");
        let text = format!("{a}  one.exe\n{b} *two.bin\nrubbish\n{a}\n  \n{}  three\n", "z".repeat(64));
        let sums = parse_sums(&text);
        assert_eq!(sums.keys().collect::<Vec<_>>(), ["one.exe", "two.bin"]);
        assert_eq!(sums["one.exe"], [0u8; 32]);
        assert_eq!(to_hex(&sums["two.bin"]), b);
    }

    #[test]
    fn a_name_appearing_twice_keeps_the_first_digest() {
        let first = "0".repeat(64);
        let second = "f".repeat(64);
        let sums = parse_sums(&format!("{first}  one.exe\n{second}  one.exe\n"));
        assert_eq!(sums["one.exe"], [0u8; 32]);
    }

    #[test]
    fn hex_round_trips_and_rejects_anything_that_is_not_32_bytes() {
        let bytes: [u8; 32] = std::array::from_fn(|i| i as u8);
        assert_eq!(from_hex(&to_hex(&bytes)), Some(bytes));
        assert_eq!(from_hex(""), None);
        assert_eq!(from_hex(&"0".repeat(63)), None);
        assert_eq!(from_hex(&"0".repeat(65)), None);
        assert_eq!(from_hex(&"g".repeat(64)), None);
    }
}
