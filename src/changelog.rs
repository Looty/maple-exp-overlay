//! What changed in each version: CHANGELOG.md, built into the exe, for the settings window's What's new page.

/// The changelog as written (see the top of CHANGELOG.md for its format).
pub const TEXT: &str = include_str!("../CHANGELOG.md");

/// One version's changes.
#[derive(Debug, Clone, PartialEq)]
pub struct Release {
    /// "0.5.0", or "Unreleased".
    pub version: String,
    /// "2026-10-29"; empty for Unreleased.
    pub date: String,
    pub sections: Vec<Section>,
}

/// "New", "Improved" or "Fixed", and its items.
#[derive(Debug, Clone, PartialEq)]
pub struct Section {
    pub title: String,
    pub items: Vec<String>,
}

impl Release {
    pub fn unreleased(&self) -> bool {
        self.version == "Unreleased"
    }
}

/// The versions in a changelog, newest first, as written. Items are plain text: a line that carries on an item is
/// joined to it, and `code` marks are dropped.
pub fn parse(text: &str) -> Vec<Release> {
    let mut out: Vec<Release> = Vec::new();
    for line in text.lines() {
        if let Some(head) = line.strip_prefix("## ") {
            let (version, date) = head.split_once(" - ").unwrap_or((head, ""));
            out.push(Release { version: version.trim().into(), date: date.trim().into(), sections: Vec::new() });
        } else if let Some(title) = line.strip_prefix("### ") {
            if let Some(r) = out.last_mut() {
                r.sections.push(Section { title: title.trim().into(), items: Vec::new() });
            }
        } else if let Some(s) = out.last_mut().and_then(|r| r.sections.last_mut()) {
            let plain = line.trim().replace('`', "");
            if let Some(item) = plain.strip_prefix("- ") {
                s.items.push(item.into());
            } else if let (false, Some(item)) = (plain.is_empty(), s.items.last_mut()) {
                item.push(' ');
                item.push_str(&plain);
            }
        }
    }
    out
}

/// The built-in changelog, newest first. Unreleased changes show only in debug builds.
pub fn releases() -> Vec<Release> {
    parse(TEXT).into_iter().filter(|r| cfg!(debug_assertions) || !r.unreleased()).collect()
}

/// "2026-10-29" -> "October 29, 2026" (as it is when it isn't such a date).
pub fn long_date(date: &str) -> String {
    const MONTHS: [&str; 12] =
        ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];
    let parts: Vec<&str> = date.split('-').collect();
    match parts[..] {
        [y, m, d] => match (y.parse::<u32>(), m.parse::<usize>(), d.parse::<u32>()) {
            (Ok(_), Ok(m @ 1..=12), Ok(d)) => format!("{} {d}, {y}", MONTHS[m - 1]),
            _ => date.into(),
        },
        _ => date.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_versions_sections_and_items() {
        let text = "# Changelog\n\nAbout this file.\n\n## Unreleased\n\n### Fixed\n- One `thing`\n  carried on.\n\n## 1.2.0 - 2026-01-05\n\n### New\n- A\n- B\n### Improved\n- C\n";
        let r = parse(text);
        assert_eq!(r.len(), 2);
        assert!(r[0].unreleased() && r[0].date.is_empty());
        assert_eq!(r[0].sections, vec![Section { title: "Fixed".into(), items: vec!["One thing carried on.".into()] }]);
        assert_eq!((r[1].version.as_str(), r[1].date.as_str()), ("1.2.0", "2026-01-05"));
        assert_eq!(r[1].sections.iter().map(|s| s.items.len()).collect::<Vec<_>>(), [2, 1]);
        assert_eq!(long_date("2026-01-05"), "January 5, 2026");
        assert_eq!(long_date("soon"), "soon");
    }

    /// The release CI posts this version's entry as the release notes and fails without one: catch that here first.
    #[test]
    fn the_changelog_has_this_version_and_is_well_formed() {
        let all = parse(TEXT);
        let version = env!("CARGO_PKG_VERSION");
        assert!(all.iter().any(|r| r.version == version), "CHANGELOG.md has no `## {version} - <date>` entry");
        for (i, r) in all.iter().enumerate() {
            assert!(!r.sections.is_empty(), "{} has no changes", r.version);
            for s in &r.sections {
                assert!(["New", "Improved", "Fixed"].contains(&s.title.as_str()), "{}: section {:?}", r.version, s.title);
                assert!(!s.items.is_empty() && s.items.iter().all(|t| !t.is_empty()), "{}: empty {}", r.version, s.title);
            }
            if r.unreleased() {
                assert_eq!(i, 0, "Unreleased goes first");
                continue;
            }
            assert_ne!(long_date(&r.date), r.date, "{}: date {:?} isn't YYYY-MM-DD", r.version, r.date);
            let num = |v: &str| v.split('.').map(|p| p.parse::<u32>().unwrap()).collect::<Vec<_>>();
            if let Some(older) = all.get(i + 1) {
                assert!(num(&r.version) > num(&older.version), "{} should come before {}", r.version, older.version);
            }
        }
    }
}
