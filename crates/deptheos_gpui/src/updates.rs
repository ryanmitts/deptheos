//! The update check: ask GitHub for the newest release and, if newer than
//! this build, offer its page. Runs at startup (unless turned off) and from
//! Check for Updates…. Nothing is downloaded here; the request carries only
//! the app's version (in the User-Agent GitHub requires).

use std::time::Duration;

use semver::Version;
use serde::Deserialize;

/// The public repository releases are published to.
pub const REPO: &str = "ryanmitts/deptheos";

/// A [`REPO`] page on GitHub: its address plus `path` ("/releases", "#readme").
pub fn repo_page(path: &str) -> String {
    format!("https://github.com/{REPO}{path}")
}

/// The website (ADR-160), where the docs are.
pub const SITE: &str = "https://deptheos.com";

/// The website's page to download the app from.
pub const DOWNLOAD_PAGE: &str = "/download/";

/// A page of the website: [`SITE`] followed by `path` ("/docs/").
pub fn site_page(path: &str) -> String {
    format!("{SITE}{path}")
}

/// A published release.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Release {
    pub version: Version,
    /// Its GitHub release page.
    pub url: String,
}

/// What a check found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    Newer(Release),
    UpToDate,
    Failed(Failure),
}

/// Why a check found nothing: the update notice words it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Failure {
    /// GitHub couldn't be reached.
    Unreachable,
    /// GitHub answered with an error (rate limit, outage); later may work.
    Refused,
    /// Its reply wasn't the release JSON.
    Unreadable,
    /// The latest release's tag (as given) isn't a version number.
    NotAVersion(String),
}

/// The release JSON fields read. Not `html_url`: the page is built from
/// [`REPO`] and the tag, so a reply can't send Download elsewhere.
#[derive(Deserialize)]
struct GithubRelease {
    tag_name: String,
}

/// This build's version.
pub fn current() -> Version {
    Version::parse(env!("CARGO_PKG_VERSION")).expect("the crate version is semver")
}

/// Ask GitHub for the newest release (no drafts or pre-releases) and compare
/// with this build. Blocking: run off the main thread.
pub fn check() -> Outcome {
    match latest() {
        Ok(Some(release)) => compare(release, &current()),
        // No release published yet.
        Ok(None) => Outcome::UpToDate,
        Err(failure) => Outcome::Failed(failure),
    }
}

fn latest() -> Result<Option<Release>, Failure> {
    let agent: ureq::Agent =
        ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(15))).build().into();
    let url = format!("https://api.github.com/repos/{REPO}/releases/latest");
    let response = agent
        .get(&url)
        .header("User-Agent", concat!("Deptheos/", env!("CARGO_PKG_VERSION")))
        .header("Accept", "application/vnd.github+json")
        .call();
    let mut response = match response {
        Ok(response) => response,
        Err(ureq::Error::StatusCode(404)) => return Ok(None),
        Err(e) => {
            tracing::warn!("update check: {e}");
            return Err(failure_of(&e));
        }
    };
    let release: GithubRelease = response.body_mut().read_json().map_err(|e| {
        tracing::warn!("update check: {e}");
        Failure::Unreadable
    })?;
    release_of(&release).map(Some)
}

/// A failed request for the notice: an HTTP status (rate limit 403 or 429)
/// is an answer, not an unreachable server.
fn failure_of(error: &ureq::Error) -> Failure {
    match error {
        ureq::Error::StatusCode(_) => Failure::Refused,
        _ => Failure::Unreachable,
    }
}

/// The release GitHub's reply describes.
fn release_of(reply: &GithubRelease) -> Result<Release, Failure> {
    let tag = reply.tag_name.trim();
    let version = parse_tag(tag).ok_or_else(|| Failure::NotAVersion(reply.tag_name.clone()))?;
    Ok(Release { version, url: release_page(tag) })
}

/// The page of release `tag` (a version, needing no escaping) on [`REPO`].
fn release_page(tag: &str) -> String {
    repo_page(&format!("/releases/tag/{tag}"))
}

/// A tag's version: `v0.2.0` or `0.2.0`, and short `v0.1` (the Swift app's)
/// as `0.1.0`.
pub fn parse_tag(tag: &str) -> Option<Version> {
    let tag = tag.trim();
    let tag = tag.strip_prefix('v').unwrap_or(tag);
    Version::parse(tag).ok().or_else(|| {
        let mut parts = tag.split('.').map(|p| p.parse::<u64>().ok());
        let major = parts.next()??;
        let minor = parts.next().unwrap_or(Some(0))?;
        parts.next().is_none().then(|| Version::new(major, minor, 0))
    })
}

fn compare(release: Release, current: &Version) -> Outcome {
    if release.version > *current { Outcome::Newer(release) } else { Outcome::UpToDate }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn release(tag: &str) -> Release {
        Release { version: parse_tag(tag).unwrap(), url: "https://example.com".into() }
    }

    /// The pages the app links to keep the addresses they had.
    #[test]
    fn repo_pages_are_under_the_repository() {
        assert_eq!(repo_page("#readme"), "https://github.com/ryanmitts/deptheos#readme");
        assert_eq!(repo_page("/releases"), "https://github.com/ryanmitts/deptheos/releases");
        assert_eq!(site_page("/docs/"), "https://deptheos.com/docs/");
    }

    /// Download opens the repository's release page, whatever the reply says.
    #[test]
    fn the_download_page_is_the_repositorys() {
        assert_eq!(
            release_page("v0.3.0"),
            "https://github.com/ryanmitts/deptheos/releases/tag/v0.3.0"
        );
        let reply = r#"{"tag_name": "v0.3.0", "html_url": "https://example.com/evil"}"#;
        let reply: GithubRelease = serde_json::from_str(reply).unwrap();
        let release = release_of(&reply).unwrap();
        assert_eq!(release.url, "https://github.com/ryanmitts/deptheos/releases/tag/v0.3.0");
        assert_eq!(release.version, Version::new(0, 3, 0));
    }

    /// An error status (403, 429, 500) differs from an unreachable server.
    #[test]
    fn an_error_status_is_not_unreachable() {
        for status in [403, 429, 500, 503] {
            assert_eq!(failure_of(&ureq::Error::StatusCode(status)), Failure::Refused);
        }
        assert_eq!(failure_of(&ureq::Error::HostNotFound), Failure::Unreachable);
        assert_eq!(failure_of(&ureq::Error::ConnectionFailed), Failure::Unreachable);
    }

    #[test]
    fn tags_parse_with_or_without_v() {
        assert_eq!(parse_tag("v0.2.0"), Some(Version::new(0, 2, 0)));
        assert_eq!(parse_tag(" 1.10.3 "), Some(Version::new(1, 10, 3)));
        assert_eq!(parse_tag("v0.1"), Some(Version::new(0, 1, 0)));
        assert_eq!(parse_tag("v2"), Some(Version::new(2, 0, 0)));
        assert_eq!(parse_tag("nightly"), None);
        assert_eq!(parse_tag("v0.1.x"), None);
    }

    #[test]
    fn only_a_newer_version_is_offered() {
        let current = Version::new(0, 2, 0);
        assert_eq!(
            compare(release("v0.10.0"), &current),
            Outcome::Newer(release("v0.10.0")),
            "compared as numbers, not text"
        );
        assert_eq!(compare(release("v0.2.0"), &current), Outcome::UpToDate);
        assert_eq!(compare(release("v0.1.9"), &current), Outcome::UpToDate);
        // A pre-release of this version is older than it.
        assert_eq!(compare(release("v0.2.0-beta.1"), &current), Outcome::UpToDate);
    }
}
