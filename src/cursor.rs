use anyhow::{Context, Result};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::process::Command;

const SANDBOX_ICON_ROOT: &str = "/run/host/share/icons";

#[derive(Debug, Clone)]
pub struct HostCursorTheme {
    xcursor_theme: Option<String>,
    xcursor_size: Option<String>,
    hyprcursor_theme: Option<String>,
    hyprcursor_size: Option<String>,
    mounts: Vec<CursorThemeMount>,
    warnings: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct CursorThemeMount {
    theme: String,
    host_path: PathBuf,
    sandbox_path: PathBuf,
}

impl HostCursorTheme {
    pub fn from_host() -> Self {
        let desktop_env = desktop_environment();
        let xcursor_theme = host_var("XCURSOR_THEME", &desktop_env);
        let xcursor_size = host_var("XCURSOR_SIZE", &desktop_env);
        let hyprcursor_theme = host_var("HYPRCURSOR_THEME", &desktop_env);
        let hyprcursor_size = host_var("HYPRCURSOR_SIZE", &desktop_env);

        let mut warnings = Vec::new();
        let mut themes = BTreeSet::new();
        for theme in [&xcursor_theme, &hyprcursor_theme].into_iter().flatten() {
            if valid_theme_name(theme) {
                themes.insert(theme.clone());
            } else {
                warnings.push(format!("ignoring cursor theme with unsafe name: {theme}"));
            }
        }

        let search_dirs = cursor_search_dirs();
        let mounts = theme_mounts(&themes, &search_dirs, &mut warnings);

        Self {
            xcursor_theme,
            xcursor_size,
            hyprcursor_theme,
            hyprcursor_size,
            mounts,
            warnings,
        }
    }

    pub fn mounts(&self) -> &[CursorThemeMount] {
        &self.mounts
    }

    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }

    pub fn describe(&self) -> Vec<String> {
        let mut lines = Vec::new();
        if let Some(theme) = &self.xcursor_theme {
            let size = self.xcursor_size.as_deref().unwrap_or("default");
            lines.push(format!("xcursor: {theme} size {size}"));
        }
        if let Some(theme) = &self.hyprcursor_theme {
            let size = self.hyprcursor_size.as_deref().unwrap_or("default");
            lines.push(format!("hyprcursor: {theme} size {size}"));
        }
        for mount in &self.mounts {
            lines.push(format!(
                "theme {}: {} -> {}",
                mount.theme,
                mount.host_path.display(),
                mount.sandbox_path.display()
            ));
        }
        lines
    }

    pub fn env(&self) -> Vec<(String, String)> {
        let mut env = Vec::new();
        push_env(&mut env, "XCURSOR_THEME", &self.xcursor_theme);
        push_env(&mut env, "XCURSOR_SIZE", &self.xcursor_size);
        push_env(&mut env, "HYPRCURSOR_THEME", &self.hyprcursor_theme);
        push_env(&mut env, "HYPRCURSOR_SIZE", &self.hyprcursor_size);
        if !self.mounts.is_empty() {
            env.push((
                "XCURSOR_PATH".to_string(),
                format!("{SANDBOX_ICON_ROOT}:/var/data/.icons:/usr/share/icons:/app/share/icons"),
            ));
        }
        env
    }
}

impl CursorThemeMount {
    pub fn host_path(&self) -> &Path {
        &self.host_path
    }

    pub fn sandbox_target_relative(&self) -> Result<PathBuf> {
        self.sandbox_path
            .strip_prefix("/")
            .map(Path::to_path_buf)
            .with_context(|| {
                format!(
                    "cursor sandbox path is not absolute: {}",
                    self.sandbox_path.display()
                )
            })
    }
}

fn host_var(name: &str, desktop_env: &BTreeMap<String, String>) -> Option<String> {
    std::env::var(name)
        .ok()
        .filter(|value| !value.is_empty())
        .or_else(|| {
            desktop_env
                .get(name)
                .filter(|value| !value.is_empty())
                .cloned()
        })
}

fn desktop_environment() -> BTreeMap<String, String> {
    let Some(pid) = hyprland_pid() else {
        return BTreeMap::new();
    };
    let Ok(output) = Command::new("procstat")
        .arg("-e")
        .arg(pid.to_string())
        .output()
    else {
        return BTreeMap::new();
    };
    if !output.status.success() {
        return BTreeMap::new();
    }
    let text = String::from_utf8_lossy(&output.stdout);
    parse_environment_tokens(&text)
}

fn hyprland_pid() -> Option<i32> {
    let output = Command::new("pgrep")
        .arg("-x")
        .arg("Hyprland")
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| line.trim().parse::<i32>().ok())
        .next()
}

fn parse_environment_tokens(text: &str) -> BTreeMap<String, String> {
    let wanted = [
        "XCURSOR_THEME",
        "XCURSOR_SIZE",
        "HYPRCURSOR_THEME",
        "HYPRCURSOR_SIZE",
    ];
    text.split_whitespace()
        .filter_map(|token| {
            let (key, value) = token.split_once('=')?;
            if wanted.contains(&key) {
                Some((key.to_string(), value.to_string()))
            } else {
                None
            }
        })
        .collect()
}

fn cursor_search_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Ok(path) = std::env::var("XCURSOR_PATH") {
        dirs.extend(
            path.split(':')
                .filter(|entry| !entry.is_empty())
                .map(PathBuf::from),
        );
    }
    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        dirs.push(home.join(".icons"));
        dirs.push(home.join(".local/share/icons"));
    }
    dirs.push(PathBuf::from("/usr/local/share/icons"));
    dirs.push(PathBuf::from("/usr/share/icons"));
    dirs
}

fn theme_mounts(
    themes: &BTreeSet<String>,
    search_dirs: &[PathBuf],
    warnings: &mut Vec<String>,
) -> Vec<CursorThemeMount> {
    let mut mounts = Vec::new();
    let mut seen = BTreeSet::new();
    let mut queue: VecDeque<String> = themes.iter().cloned().collect();

    while let Some(theme) = queue.pop_front() {
        if !seen.insert(theme.clone()) {
            continue;
        }
        let Some(host_path) = find_theme_dir(&theme, search_dirs) else {
            if themes.contains(&theme) {
                warnings.push(format!("cursor theme {theme} was not found on the host"));
            }
            continue;
        };

        for inherited in inherited_themes(&host_path) {
            if valid_theme_name(&inherited) && !seen.contains(&inherited) {
                queue.push_back(inherited);
            }
        }

        mounts.push(CursorThemeMount {
            sandbox_path: PathBuf::from(format!("{SANDBOX_ICON_ROOT}/{theme}")),
            theme,
            host_path,
        });
    }

    mounts
}

fn find_theme_dir(theme: &str, search_dirs: &[PathBuf]) -> Option<PathBuf> {
    search_dirs
        .iter()
        .map(|dir| dir.join(theme))
        .find(|path| path.join("cursors").is_dir())
}

fn inherited_themes(theme_dir: &Path) -> Vec<String> {
    for file in ["index.theme", "cursor.theme"] {
        let path = theme_dir.join(file);
        let Ok(text) = fs::read_to_string(path) else {
            continue;
        };
        for line in text.lines() {
            let line = line.trim();
            let Some(value) = line.strip_prefix("Inherits=") else {
                continue;
            };
            return value
                .split(',')
                .map(str::trim)
                .map(|theme| theme.trim_matches('"').trim_matches('\''))
                .filter(|theme| !theme.is_empty())
                .map(ToOwned::to_owned)
                .collect();
        }
    }
    Vec::new()
}

fn valid_theme_name(theme: &str) -> bool {
    let mut components = Path::new(theme).components();
    matches!(components.next(), Some(Component::Normal(_))) && components.next().is_none()
}

fn push_env(env: &mut Vec<(String, String)>, key: &str, value: &Option<String>) {
    if let Some(value) = value {
        env.push((key.to_string(), value.clone()));
    }
}

#[cfg(test)]
mod tests {
    use super::{parse_environment_tokens, valid_theme_name};

    #[test]
    fn parses_cursor_vars_from_procstat_text() {
        let env = parse_environment_tokens(
            "PID COMM ENVIRONMENT\n1 Hyprland XCURSOR_THEME=Bibata XCURSOR_SIZE=24 HYPRCURSOR_THEME=Bibata HYPRCURSOR_SIZE=24 PATH=/bin\n",
        );
        assert_eq!(env.get("XCURSOR_THEME").map(String::as_str), Some("Bibata"));
        assert_eq!(env.get("HYPRCURSOR_SIZE").map(String::as_str), Some("24"));
        assert!(!env.contains_key("PATH"));
    }

    #[test]
    fn validates_theme_names_as_single_path_component() {
        assert!(valid_theme_name("Bibata-Modern-Classic"));
        assert!(!valid_theme_name("../Bibata"));
        assert!(!valid_theme_name("parent/child"));
        assert!(!valid_theme_name(""));
    }
}
