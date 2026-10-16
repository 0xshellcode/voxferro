//! Recently opened and saved files, kept between sessions.
//!
//! A plain text file, one path per line, newest first, in the platform's
//! place for app settings: `~/Library/Application Support/Voxferro` on
//! macOS, `%APPDATA%\Voxferro` on Windows, `$XDG_CONFIG_HOME/voxferro` (or
//! `~/.config/voxferro`) elsewhere. Losing it loses nothing but the list,
//! so every failure is silent.

use std::path::PathBuf;

/// How many paths the list keeps.
pub const LIMIT: usize = 10;

fn file() -> Option<PathBuf> {
    let dir = if cfg!(target_os = "macos") {
        PathBuf::from(std::env::var_os("HOME")?).join("Library/Application Support/Voxferro")
    } else if cfg!(windows) {
        PathBuf::from(std::env::var_os("APPDATA")?).join("Voxferro")
    } else {
        match std::env::var_os("XDG_CONFIG_HOME") {
            Some(d) => PathBuf::from(d).join("voxferro"),
            None => PathBuf::from(std::env::var_os("HOME")?).join(".config/voxferro"),
        }
    };
    Some(dir.join("recent.txt"))
}

/// The list, newest first.
pub fn load() -> Vec<String> {
    file()
        .and_then(|f| std::fs::read_to_string(f).ok())
        .map(|text| parse(&text))
        .unwrap_or_default()
}

fn parse(text: &str) -> Vec<String> {
    text.lines().map(str::trim).filter(|l| !l.is_empty()).take(LIMIT).map(str::to_owned).collect()
}

/// `list` with `path` moved to the front, without duplicates, trimmed to
/// the limit.
pub fn with(list: &[String], path: &str) -> Vec<String> {
    let path = std::fs::canonicalize(path)
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| path.to_owned());
    let mut out = vec![path.clone()];
    out.extend(list.iter().filter(|p| **p != path).cloned());
    out.truncate(LIMIT);
    out
}

/// Records `path` as the most recent and saves the list.
pub fn push(list: &mut Vec<String>, path: &str) {
    *list = with(list, path);
    if let Some(f) = file() {
        if let Some(dir) = f.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(f, list.join("\n") + "\n");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newest_first_without_duplicates_and_limited() {
        let mut list: Vec<String> = (0..LIMIT).map(|i| format!("/m/{i}.vox")).collect();
        list = with(&list, "/m/3.vox");
        assert_eq!(list[0], "/m/3.vox");
        assert_eq!(list.iter().filter(|p| *p == "/m/3.vox").count(), 1);
        list = with(&list, "/m/new.vox");
        assert_eq!(list.len(), LIMIT);
        assert_eq!(list[0], "/m/new.vox");
        assert_eq!(parse("a\n\n b \n"), vec!["a".to_owned(), "b".to_owned()]);
    }
}
