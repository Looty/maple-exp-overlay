//! In-app updates, the platform-neutral part: which release is newer, where its files are, and whether the
//! downloaded exe really comes from us.
//!
//! Each GitHub release carries the bare exe and a minisign signature of it (`<exe>.minisig`, made in CI with the
//! project's secret key). The public key is built into the app, so a release that wasn't signed with that key is
//! refused, even if someone could upload files to the repository. The signature's trusted comment names the
//! release (`maple-exp-overlay v1.2.3`), so an old signed exe can't be passed off as a newer version.
//!
//! The download and the swap of the running exe are Windows code, in the binary (`updater.rs`).

use serde_json::Value;

/// The GitHub repository releases come from.
pub const REPO: &str = "Looty/maple-exp-overlay";
/// The release asset that is the program itself, and its signature next to it.
pub const EXE_ASSET: &str = "maple-exp-overlay.exe";
/// The minisign public key releases are signed with (the line after "untrusted comment:" in the `.pub` file).
/// Empty: this build has no key, and updating is switched off. (key id 30E1AFEC78E4FC10)
pub const PUBLIC_KEY: &str = "RWQQ/OR47K/hMBs+NkWZtEviYoTcxKvHlZr9LWKVWLEnJ3P/9QnDxI1W";
/// What the trusted comment of a release's signature says, for the release `tag`.
pub fn signed_comment(tag: &str) -> String {
    format!("maple-exp-overlay {tag}")
}

/// A release that can be installed.
#[derive(Debug, Clone, PartialEq)]
pub struct Release {
    pub tag: String,
    pub exe_url: String,
    pub sig_url: String,
}

/// "v1.2.3" or "1.2.3" as numbers; None for anything else (pre-release suffixes included: those are never offered).
pub fn parse_version(s: &str) -> Option<(u32, u32, u32)> {
    let mut it = s.trim().trim_start_matches('v').split('.').map(|p| p.parse::<u32>().ok());
    let v = (it.next()??, it.next()??, it.next()??);
    it.next().is_none().then_some(v)
}

/// The release tagged `tag` is newer than the running `current` version.
pub fn is_newer(tag: &str, current: &str) -> bool {
    matches!((parse_version(tag), parse_version(current)), (Some(t), Some(c)) if t > c)
}

/// Reads GitHub's "latest release" answer: its tag and the download links of the exe and its signature.
pub fn parse_latest(json: &[u8]) -> Result<Release, String> {
    let v: Value = serde_json::from_slice(json).map_err(|_| "GitHub's answer couldn't be read".to_string())?;
    let tag = v["tag_name"].as_str().ok_or("the latest release has no version tag")?.to_string();
    let url = |name: &str| {
        v["assets"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|a| a["name"] == name)
            .and_then(|a| a["browser_download_url"].as_str())
            .map(str::to_string)
    };
    let sig_name = format!("{EXE_ASSET}.minisig");
    match (url(EXE_ASSET), url(&sig_name)) {
        (Some(exe_url), Some(sig_url)) => Ok(Release { tag, exe_url, sig_url }),
        _ => Err(format!("release {tag} has no signed program to download")),
    }
}

/// Checks that `exe` is signed by `public_key` (base64) with `sig` (the `.minisig` text), for the release `tag`.
pub fn verify(exe: &[u8], sig: &str, public_key: &str, tag: &str) -> Result<(), String> {
    use minisign_verify::{PublicKey, Signature};
    let key = PublicKey::from_base64(public_key.trim()).map_err(|_| "this build has no valid update key".to_string())?;
    let sig = Signature::decode(sig.trim()).map_err(|_| "the release's signature couldn't be read".to_string())?;
    key.verify(exe, &sig, false).map_err(|_| "the download isn't signed by this app's key: not installed".to_string())?;
    if sig.trusted_comment() != signed_comment(tag) {
        return Err(format!("the signature is for \"{}\", not {tag}: not installed", sig.trusted_comment()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions() {
        assert_eq!(parse_version("v0.3.0"), Some((0, 3, 0)));
        assert_eq!(parse_version("1.10.2"), Some((1, 10, 2)));
        assert_eq!(parse_version("v1.2"), None);
        assert_eq!(parse_version("v1.2.3-beta"), None);
        assert_eq!(parse_version("v1.2.3.4"), None);
        assert!(is_newer("v0.3.1", "0.3.0"));
        assert!(is_newer("v0.10.0", "0.9.9")); // numbers, not text
        assert!(!is_newer("v0.3.0", "0.3.0"));
        assert!(!is_newer("v0.2.9", "0.3.0"));
        assert!(!is_newer("nonsense", "0.3.0"));
    }

    #[test]
    fn latest_release() {
        let json = br#"{"tag_name":"v0.4.0","assets":[
            {"name":"maple-exp-overlay-v0.4.0-windows-x64.zip","browser_download_url":"https://x/zip"},
            {"name":"maple-exp-overlay.exe","browser_download_url":"https://x/exe"},
            {"name":"maple-exp-overlay.exe.minisig","browser_download_url":"https://x/sig"}]}"#;
        let r = parse_latest(json).unwrap();
        assert_eq!(r, Release { tag: "v0.4.0".into(), exe_url: "https://x/exe".into(), sig_url: "https://x/sig".into() });
        // an older release without the signed exe can't be installed
        let unsigned = br#"{"tag_name":"v0.3.0","assets":[{"name":"maple-exp-overlay.exe","browser_download_url":"https://x/exe"}]}"#;
        assert!(parse_latest(unsigned).is_err());
        assert!(parse_latest(b"not json").is_err());
    }

    /// "hello", signed with the project's release key the way CI signs a release (trusted comment for v9.9.9).
    const HELLO_SIG: &str = "untrusted comment: signature from minisign secret key
RUQQ/OR47K/hMGN7wcel92q2dwFZ5VCPrUpHg1Dw2VWkIntCxkkUSp73HsEtKJEZTfoM0qed+0qAD1XO21Kx5bnU8cT7mw4qTA4=
trusted comment: maple-exp-overlay v9.9.9
8jHqIOlaNI7oG9p6TeQMm3VTzsf175Mx7BrBDUAZ5EY1r+FpQHz5SEmunLJCqV+/4wqTyLqSe3aWT2IX4zCUAQ==";

    #[test]
    fn accepts_what_we_signed() {
        assert_eq!(verify(b"hello", HELLO_SIG, PUBLIC_KEY, "v9.9.9"), Ok(()));
        // the signature names v9.9.9: it can't install as any other version
        assert!(verify(b"hello", HELLO_SIG, PUBLIC_KEY, "v9.9.8").is_err());
        // one changed byte and it's refused
        assert!(verify(b"hellp", HELLO_SIG, PUBLIC_KEY, "v9.9.9").is_err());
        // so is a signature from another key
        assert!(verify(b"hello", HELLO_SIG, "RWQf6LRCGA9i53mlYecO4IzT51TGPpvWucNSCh1CBM0QTaLn73Y7GFO3", "v9.9.9").is_err());
    }

    #[test]
    fn refuses_what_isnt_signed() {
        // (the minisign project's own test key and a signature of something else)
        let key = "RWQf6LRCGA9i53mlYecO4IzT51TGPpvWucNSCh1CBM0QTaLn73Y7GFO3";
        let sig = "untrusted comment: signature from minisign secret key
RWQf6LRCGA9i59SLOFxz6NxvASXDJeRtuZykwQepbDEGt87ig1BNpWaVWuNrm73YiIiJbq71Wi+dP9eKL8OC351vwIasSSbXxwA=
trusted comment: timestamp:1555779966\tfile:test
QtKMXWyYcwdpZAlPF7tE2ENJkRd1ujvKjlj1m9RtHTBnZPa5WKU5uWRs5GoP5M/VqE81QFuMKI5k/SfNQUaOAA==";
        assert!(verify(b"a tampered exe", sig, key, "v1.0.0").is_err());
        assert!(verify(b"test", "garbage", key, "v1.0.0").is_err());
        assert!(verify(b"test", sig, "", "v1.0.0").is_err()); // no key built in: never installs
    }
}
