//! A folder as the desktop shows it, with the home folder as "~".

use std::path::Path;

/// "~/Macro/spider": `dir` with the user's home folder
/// ([`dirs::home_dir`]) as "~", in the platform's own separators.
pub(crate) fn short_dir(dir: &Path) -> String {
    short_dir_under(dir, dirs::home_dir().as_deref())
}

/// [`short_dir`] with `home` as the home folder.
fn short_dir_under(dir: &Path, home: Option<&Path>) -> String {
    match home.and_then(|home| dir.strip_prefix(home).ok()) {
        Some(rest) if rest.as_os_str().is_empty() => "~".into(),
        Some(rest) => Path::new("~").join(rest).display().to_string(),
        None => dir.display().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    #[test]
    fn the_home_folder_is_a_tilde() {
        let users = PathBuf::from("/Users");
        let home = users.join("someone");
        let at = |dir: &Path| short_dir_under(dir, Some(&home));
        let tilde = |rest: &[&str]| {
            rest.iter().fold(PathBuf::from("~"), |p, r| p.join(r)).display().to_string()
        };
        assert_eq!(at(&home), "~");
        assert_eq!(at(&home.join("Macro").join("spider")), tilde(&["Macro", "spider"]));
        let beside = users.join("someonelse").join("Macro");
        assert_eq!(at(&beside), beside.display().to_string(), "not a prefix of a name");
        assert_eq!(short_dir_under(&home, None), home.display().to_string());
    }

    /// The home folder is the platform's, not only `HOME`'s, which Windows
    /// doesn't set.
    #[test]
    fn the_home_folder_is_the_platforms() {
        let home = dirs::home_dir().expect("a home folder");
        assert_eq!(
            short_dir(&home.join("Macro")),
            Path::new("~").join("Macro").display().to_string()
        );
    }
}
