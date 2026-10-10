//! Project links (Help menu, About dialog, Home screen).
//!
//! Jilo Studio has a page on rustamguloff.com (in the interface's language where the site has it)
//! and a public GitHub repository, a fork of FilmCraft by the ArtCraft team, which is credited by
//! name (`help.upstream`).

/// The app's short name, used in the website and repository URLs.
pub const APP: &str = "jilo-studio";

/// Jilo Studio's page on rustamguloff.com, in English (the site puts the language in the path:
/// `/en/…`, `/ru/…`; Tajik, the site's default, has none).
pub const WEBSITE: &str = "https://rustamguloff.com/en/page/projects/jilo-studio";
/// The same page in Russian.
pub const WEBSITE_RU: &str = "https://rustamguloff.com/ru/page/projects/jilo-studio";
/// Jilo Studio's source repository.
pub const GITHUB: &str = "https://github.com/guloff/jilo-studio";
/// Issues of Jilo Studio's repository.
pub const ISSUES: &str = "https://github.com/guloff/jilo-studio/issues";
/// The project Jilo Studio is a fork of.
pub const UPSTREAM: &str = "https://github.com/storytold/filmcraft";

/// (command id, label, url) for every link, in menu order. Labels say "FilmCraft" as upstream's
/// do; the interface shows them as Jilo Studio (`brand::rebrand`), except the credit.
pub const ALL: [(&str, &str, &str); 4] = [
    ("help.website", "FilmCraft Website", WEBSITE),
    ("help.github", "FilmCraft on GitHub", GITHUB),
    ("help.reportIssue", "Report an Issue…", ISSUES),
    ("help.upstream", "Based on FilmCraft by the ArtCraft Team", UPSTREAM),
];

/// The website in the interface's language (Russian, else English).
pub fn website() -> &'static str {
    if crate::i18n::current() == crate::i18n::Language::Ru { WEBSITE_RU } else { WEBSITE }
}

/// The URL a `help.*` link command opens.
pub fn url_for(command: &str) -> Option<&'static str> {
    if command == "help.website" {
        return Some(website());
    }
    ALL.iter().find(|(id, _, _)| *id == command).map(|(_, _, u)| *u)
}

/// Open `url` in the system browser (a new tab on the web).
pub fn open(ctx: &egui::Context, url: &str) {
    ctx.open_url(egui::OpenUrl::new_tab(url));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_follow_the_site_and_repository_scheme() {
        assert_eq!(WEBSITE, format!("https://rustamguloff.com/en/page/projects/{APP}"));
        assert_eq!(WEBSITE_RU, WEBSITE.replace("/en/", "/ru/"));
        assert_eq!(GITHUB, format!("https://github.com/guloff/{APP}"));
        assert_eq!(ISSUES, format!("{GITHUB}/issues"));
        assert!(ALL.iter().all(|(id, _, u)| id.starts_with("help.") && u.starts_with("https://")));
        assert!(!ALL.iter().any(|(_, _, u)| u.contains("artcraft") || u.contains("discord")), "no ArtCraft links: their marks stay out of this build");
        assert_eq!(url_for("help.upstream"), Some(UPSTREAM));
        assert_eq!(url_for("help.nope"), None);
    }
}
