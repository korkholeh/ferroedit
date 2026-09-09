//! Checking GitHub for a newer release, off the UI thread.
//!
//! The editor cannot update itself: it ships as one binary that `install.sh`
//! puts in place (ADR-060), and a program that rewrites its own file while it
//! is running is a much larger promise than "there is a 0.2.0". So everything
//! here answers one question — is the newest published release newer than this
//! build? — and the answer is a sentence, not an action.
//!
//! There is no HTTP client in the dependency tree and this does not add one.
//! `curl` and `wget` are what `install.sh` already requires of the machine
//! (ADR-060), and shelling out to one of them is the same trade ADR-001 made
//! for git: no TLS stack in the binary, nothing that breaks a static musl
//! build (ADR-003), and a subprocess that is easy to reason about.

use std::fmt;
use std::process::{Command, Stdio};
use std::sync::mpsc::Sender;
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::event::AppEvent;

/// What this build calls itself, which is the left-hand side of every
/// comparison here.
pub const CURRENT: &str = env!("CARGO_PKG_VERSION");

/// Where a reader is sent to get the new build. It is the whole of what the
/// editor can offer, so it goes in the message rather than in the log.
pub const RELEASES_URL: &str = "https://github.com/korkholeh/ferroedit/releases";

/// The redirect `curl` is pointed at: GitHub answers it with the tag of the
/// newest published release.
const LATEST_URL: &str = "https://github.com/korkholeh/ferroedit/releases/latest";

/// The same question asked of the API, which is what `wget` has to use.
const API_URL: &str = "https://api.github.com/repos/korkholeh/ferroedit/releases/latest";

/// Seconds a fetcher is given before it gives up. The check is background work
/// nobody is waiting on, but a request that never returns would hold a thread
/// and a subprocess for the rest of the session.
const TIMEOUT_SECS: &str = "10";

/// How long an automatic check is good for.
///
/// A day rather than a run: the editor is opened dozens of times a day in a
/// terminal, `api.github.com` allows sixty unauthenticated calls an hour per
/// address, and a release that appeared four minutes ago is not news that
/// cannot wait. It also bounds the noise — the most a user can be told about
/// one release without asking is once a day.
const INTERVAL_SECS: u64 = 24 * 60 * 60;

/// A `major.minor.patch` release number.
///
/// Strictly three numbers, with an optional `v` in front because that is how
/// the tags are written. A tag with anything else on it — `v0.2.0-rc.1` — does
/// not parse and is reported as a check that could not work out the answer:
/// GitHub's "latest" never points at a pre-release, so a suffix here means the
/// repository is doing something this build was not told about, and guessing
/// which side of `0.2.0` an `rc.1` falls on is how an editor tells a user to
/// upgrade to something older than what they have.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version {
    major: u64,
    minor: u64,
    patch: u64,
}

impl Version {
    pub fn parse(text: &str) -> Option<Self> {
        let text = text.trim();
        let text = text.strip_prefix('v').unwrap_or(text);
        let mut parts = text.split('.');
        let mut number = || parts.next()?.parse::<u64>().ok();
        let (major, minor, patch) = (number()?, number()?, number()?);
        // A fourth component is not a version this build understands, and
        // ignoring it would make `0.2.0.1` compare equal to `0.2.0`.
        parts.next().is_none().then_some(Self {
            major,
            minor,
            patch,
        })
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

/// What a finished check found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateOutcome {
    /// A published release newer than this build.
    Available {
        latest: String,
    },
    UpToDate,
    /// The check did not get an answer. The string is the half worth showing:
    /// no fetcher on the machine, no network, a tag that did not parse.
    Failed(String),
}

/// A finished check on its way back to the main loop.
///
/// `asked_for` is the whole of the difference between the two kinds of check,
/// and it is carried rather than looked up because by the time the answer
/// arrives the settings may have been changed under it. A check the user asked
/// for owes an answer either way; one that ran by itself at start-up only
/// speaks when it has news (ADR-065).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateCheck {
    pub asked_for: bool,
    pub outcome: UpdateOutcome,
}

/// The news itself: what the dialog puts in its title bar.
pub fn headline(latest: &str) -> String {
    format!("FerroEdit {latest} is available")
}

/// The same news as one status-bar line (SPEC §39).
///
/// It carries the action rather than the URL. The status bar is the first
/// thing a narrow terminal takes columns from (ADR-039), and half a URL is
/// worse than no URL — where the dialog has a box it can size to the address,
/// this has whatever is left beside the readouts. Re-running the install line
/// is what upgrades an installed FerroEdit anyway; the address is for the
/// reader who wants to look first, and it is in the dialog.
pub fn announcement(latest: &str) -> String {
    format!("FerroEdit {latest} is out — re-run the install script")
}

/// Runs a check on a thread of its own and answers on the main loop's channel.
///
/// The same shape as the git worker (ADR-033) and for the same reason: the
/// answer wakes the loop exactly as a key press does, so nothing polls and
/// nothing blocks a frame. It is detached — there is no cancelling it, and
/// nothing depends on it having finished.
pub fn spawn(events: Sender<AppEvent>, asked_for: bool) {
    let started = thread::Builder::new().name("update".into()).spawn(move || {
        let outcome = check();
        log::info!("update check finished: {outcome:?}");
        // The editor has shut down; there is nobody left to tell.
        let _ = events.send(AppEvent::UpdateChecked(UpdateCheck { asked_for, outcome }));
    });
    if let Err(err) = started {
        // Not worth failing a run over: an editor that would not start because
        // it could not ask about its own version would be a worse editor than
        // one that is a release behind.
        log::warn!("could not start the update check: {err}");
    }
}

/// Whether an automatic check is due, given when the last one answered.
///
/// A timestamp in the future is due rather than never: a config written on a
/// machine whose clock was wrong must not switch the check off for good.
pub fn is_due(last_check: Option<u64>, now: u64) -> bool {
    match last_check {
        None => true,
        Some(then) => now < then || now - then >= INTERVAL_SECS,
    }
}

/// Now, in seconds since the epoch. `None` on a clock set before 1970, which
/// is a machine whose timestamps are not worth writing down.
pub fn now_secs() -> Option<u64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|since| since.as_secs())
}

fn check() -> UpdateOutcome {
    match fetch_latest_tag() {
        Ok(tag) => compare(&tag, CURRENT),
        Err(reason) => UpdateOutcome::Failed(reason),
    }
}

/// The judgement, with the network taken out of it.
///
/// Split from `check` so the part with the decision in it is testable: what
/// this build does when GitHub says `v0.2.0` must not be something only a
/// machine with a network can find out.
pub fn compare(tag: &str, current: &str) -> UpdateOutcome {
    let Some(latest) = Version::parse(tag) else {
        return UpdateOutcome::Failed(format!("GitHub answered {tag:?}, which is not a version"));
    };
    let Some(current) = Version::parse(current) else {
        return UpdateOutcome::Failed(format!("this build calls itself {current:?}"));
    };
    if latest > current {
        UpdateOutcome::Available {
            // The parsed form, not the tag: the editor names itself `0.2.0`
            // everywhere else, and `v0.2.0` in one sentence out of ten reads
            // as a different kind of thing.
            latest: latest.to_string(),
        }
    } else {
        UpdateOutcome::UpToDate
    }
}

/// The two fetchers `install.sh` accepts, in the order it prefers them.
enum Fetcher {
    Curl,
    Wget,
}

fn detect_fetcher() -> Option<Fetcher> {
    for (program, fetcher) in [("curl", Fetcher::Curl), ("wget", Fetcher::Wget)] {
        // `--version` rather than `which`: what matters is that it runs, and
        // this is the same question with one fewer program in the answer.
        let found = Command::new(program)
            .arg("--version")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok();
        if found {
            return Some(fetcher);
        }
    }
    None
}

fn fetch_latest_tag() -> Result<String, String> {
    match detect_fetcher() {
        Some(Fetcher::Curl) => curl_tag(),
        Some(Fetcher::Wget) => wget_tag(),
        None => Err("neither curl nor wget is installed".into()),
    }
}

/// The tag read out of the redirect on `/releases/latest`.
///
/// The same trick `install.sh` uses, for the reason it gives: the API allows
/// sixty unauthenticated calls an hour per address, which a shared NAT can
/// have spent already, and the editor must not report someone else's rate
/// limit as its own failure. A `HEAD` that follows the redirect answers the
/// question with no JSON and no quota.
fn curl_tag() -> Result<String, String> {
    let body = run(
        "curl",
        &[
            "-fsSLI",
            "-o",
            "/dev/null",
            "-w",
            "%{url_effective}",
            "--proto",
            "=https",
            "--tlsv1.2",
            "--max-time",
            TIMEOUT_SECS,
            LATEST_URL,
        ],
    )?;
    let url = String::from_utf8_lossy(&body);
    tag_in_url(&url).ok_or_else(|| format!("GitHub redirected to {:?}", url.trim()))
}

/// The last path segment of the URL the redirect landed on.
fn tag_in_url(url: &str) -> Option<String> {
    let tag = url.trim().trim_end_matches('/').rsplit('/').next()?;
    // `/releases/latest` redirecting to itself means there is no release yet.
    (!tag.is_empty() && tag != "latest").then(|| tag.to_string())
}

/// `wget` cannot report a redirect target usefully, so it asks the API and
/// pays the quota — the same fallback `install.sh` has.
fn wget_tag() -> Result<String, String> {
    let body = run(
        "wget",
        &[
            "-q",
            "-O",
            "-",
            "--timeout",
            TIMEOUT_SECS,
            "--tries",
            "1",
            API_URL,
        ],
    )?;
    tag_in_json(&body)
}

fn tag_in_json(body: &[u8]) -> Result<String, String> {
    let json: serde_json::Value = serde_json::from_slice(body)
        .map_err(|err| format!("GitHub's answer did not parse: {err}"))?;
    json.get("tag_name")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| "GitHub's answer named no release".to_string())
}

/// Runs a fetcher and hands back its stdout.
///
/// Blocking, on the check's own thread: the `--max-time` and `--timeout` flags
/// are what bound it, so there is no kill timer here of the kind the git
/// service needs (ADR-030). A child that outlives them anyway costs one thread
/// and nothing else — the UI is never waiting on this.
fn run(program: &str, args: &[&str]) -> Result<Vec<u8>, String> {
    let output = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .map_err(|err| format!("could not run {program}: {err}"))?;
    if !output.status.success() {
        let said = String::from_utf8_lossy(&output.stderr);
        let reason = said.lines().next().unwrap_or("").trim();
        return Err(if reason.is_empty() {
            format!("{program} could not reach GitHub")
        } else {
            reason.to_string()
        });
    }
    Ok(output.stdout)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tag_parses_with_or_without_its_v() {
        assert_eq!(Version::parse("v1.2.3"), Version::parse("1.2.3"));
        assert_eq!(Version::parse(" v1.2.3\n"), Version::parse("1.2.3"));
        assert_eq!(Version::parse("1.2.3").unwrap().to_string(), "1.2.3");
    }

    /// Guessing which side of `0.2.0` an `rc.1` falls on is how an editor
    /// tells a user to upgrade to something older than what they have.
    #[test]
    fn anything_that_is_not_three_numbers_is_not_a_version() {
        for text in ["", "v", "1.2", "1.2.3.4", "1.2.x", "v0.2.0-rc.1", "latest"] {
            assert!(Version::parse(text).is_none(), "{text} parsed");
        }
    }

    #[test]
    fn versions_order_by_each_component_in_turn() {
        let v = |text| Version::parse(text).unwrap();
        assert!(v("0.2.0") > v("0.1.9"));
        assert!(v("1.0.0") > v("0.99.99"));
        assert!(v("0.1.10") > v("0.1.9"));
        assert_eq!(v("0.1.3"), v("v0.1.3"));
    }

    #[test]
    fn a_newer_release_is_available_and_the_same_one_is_not() {
        assert_eq!(
            compare("v0.2.0", "0.1.3"),
            UpdateOutcome::Available {
                latest: "0.2.0".into()
            }
        );
        assert_eq!(compare("v0.1.3", "0.1.3"), UpdateOutcome::UpToDate);
    }

    /// A build from a branch is ahead of the newest tag, and being told to
    /// downgrade to it would be worse than being told nothing.
    #[test]
    fn a_build_ahead_of_the_latest_release_is_up_to_date() {
        assert_eq!(compare("v0.1.3", "0.2.0"), UpdateOutcome::UpToDate);
    }

    #[test]
    fn a_tag_that_is_not_a_version_is_a_failure_rather_than_an_upgrade() {
        let UpdateOutcome::Failed(reason) = compare("nightly", "0.1.3") else {
            panic!("a tag that does not parse cannot be compared");
        };
        assert!(reason.contains("nightly"), "{reason}");
    }

    #[test]
    fn the_first_check_is_due_and_the_next_one_is_a_day_later() {
        assert!(is_due(None, 0));
        assert!(!is_due(Some(1_000), 1_000 + INTERVAL_SECS - 1));
        assert!(is_due(Some(1_000), 1_000 + INTERVAL_SECS));
    }

    /// A config written on a machine whose clock was wrong must not switch the
    /// check off for good.
    #[test]
    fn a_timestamp_in_the_future_is_due_rather_than_never() {
        assert!(is_due(Some(2_000), 1_000));
    }

    #[test]
    fn the_tag_is_the_last_segment_of_the_redirect() {
        assert_eq!(
            tag_in_url("https://github.com/korkholeh/ferroedit/releases/tag/v0.2.0"),
            Some("v0.2.0".into())
        );
        assert_eq!(
            tag_in_url("https://github.com/korkholeh/ferroedit/releases/tag/v0.2.0/\n"),
            Some("v0.2.0".into())
        );
    }

    /// A repository with no release redirects `/releases/latest` to itself,
    /// which is an answer and not a tag.
    #[test]
    fn a_redirect_that_named_no_release_is_not_a_tag() {
        assert_eq!(tag_in_url(""), None);
        assert_eq!(
            tag_in_url("https://github.com/korkholeh/ferroedit/releases/latest"),
            None
        );
    }

    #[test]
    fn the_api_answer_is_read_for_its_tag_name() {
        let body = br#"{"tag_name": "v0.2.0", "name": "0.2.0", "draft": false}"#;
        assert_eq!(tag_in_json(body).unwrap(), "v0.2.0");
    }

    #[test]
    fn an_answer_with_no_release_in_it_is_a_failure() {
        assert!(tag_in_json(b"{}").is_err());
        assert!(tag_in_json(b"not json").is_err());
    }

    /// Both forms name the version, and neither is long enough to be cut in
    /// half by a narrow terminal — which is why the URL is in the dialog,
    /// where there is a box to size to it, and not on the status bar.
    #[test]
    fn both_forms_of_the_news_name_the_version_and_stay_short() {
        for said in [headline("0.2.0"), announcement("0.2.0")] {
            assert!(said.contains("0.2.0"), "{said}");
            assert!(
                said.chars().count() <= 56,
                "{said} is {}",
                said.chars().count()
            );
        }
        assert!(RELEASES_URL.chars().count() <= 48, "{RELEASES_URL}");
    }

    /// The version in `Cargo.toml` is what every comparison starts from, so a
    /// build that cannot parse its own name would report a failure to everyone.
    #[test]
    fn this_build_knows_its_own_version() {
        assert!(Version::parse(CURRENT).is_some(), "{CURRENT}");
    }
}
