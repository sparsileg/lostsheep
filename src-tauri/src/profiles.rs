// profiles.rs — issue #85: registry of congregation profiles and the path
// resolution that lets the rest of the app stay ignorant of multi-profile
// support. `commands/profiles.rs` is the thin Tauri-command layer on top
// of this; this module is plain Rust so it's testable without a running
// app.
//
// Compatibility path: when no registry.json exists yet, or it exists but
// has no active profile, resolve_data_paths() returns the exact same flat
// paths main.rs always has (`data_dir/lost-sheep.db`,
// `data_dir/roads.db`). Nothing about today's single-congregation
// behavior changes until a profile is actually created and made active —
// existing installs keep working unmodified after this ships.

use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Profile {
    pub slug: String,
    pub name: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Registry {
    #[serde(default)]
    pub profiles: Vec<Profile>,
    #[serde(default)]
    pub active: Option<String>,
}

fn registry_path(data_dir: &Path) -> PathBuf {
    data_dir.join("registry.json")
}

/// Never fails outright — a missing or corrupt registry.json just means
/// "no profiles yet," which is a normal, common state (every install
/// before this feature, and every fresh install after it until the user
/// creates a profile). Falling back to Registry::default() here is what
/// keeps main.rs's setup() on the flat-path compatibility branch instead
/// of refusing to start.
pub fn load_registry(data_dir: &Path) -> Registry {
    let path = registry_path(data_dir);
    let Ok(raw) = std::fs::read_to_string(&path) else {
        return Registry::default();
    };
    serde_json::from_str(&raw).unwrap_or_default()
}

/// Atomic write: temp file in the same directory (so the rename is same-
/// filesystem and therefore atomic on every platform this app targets),
/// then rename over the real path. A crash or power loss mid-write leaves
/// either the old registry.json or nothing — never a half-written file
/// the next launch would fail to parse.
pub fn save_registry(data_dir: &Path, registry: &Registry) -> anyhow::Result<()> {
    std::fs::create_dir_all(data_dir)?;
    let final_path = registry_path(data_dir);
    let tmp_path = data_dir.join("registry.json.tmp");
    let json = serde_json::to_string_pretty(registry)?;
    {
        let mut f = std::fs::File::create(&tmp_path)?;
        f.write_all(json.as_bytes())?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp_path, &final_path)?;
    Ok(())
}

/// Lowercase, any run of whitespace/dash/underscore collapses to a single
/// dash, anything else that isn't ASCII alphanumeric is dropped.
/// "Winchester" -> "winchester", "Woodstock (evening)" -> "woodstock-evening".
pub fn slugify(name: &str) -> String {
    let mut out = String::new();
    let mut last_was_dash = false;
    for ch in name.trim().chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            last_was_dash = false;
        } else if (ch.is_whitespace() || ch == '-' || ch == '_') && !last_was_dash && !out.is_empty() {
            out.push('-');
            last_was_dash = true;
        }
        // anything else (punctuation, etc.) is dropped silently
    }
    while out.ends_with('-') {
        out.pop();
    }
    out
}

/// Where the given profile's database files live, creating the containing
/// directory if this is the first time it's been resolved.
pub fn profile_dir(data_dir: &Path, slug: &str) -> std::io::Result<PathBuf> {
    let dir = data_dir.join("profiles").join(slug);
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// The single decision point between "legacy flat layout" and "this
/// profile's own directory." See the module doc comment above for why the
/// no-active-profile branch must stay byte-for-byte identical to main.rs's
/// pre-#85 paths.
pub fn resolve_data_paths(
    data_dir: &Path,
    registry: &Registry,
) -> anyhow::Result<(PathBuf, PathBuf, Option<String>)> {
    if let Some(active_slug) = &registry.active {
        if let Some(profile) = registry.profiles.iter().find(|p| &p.slug == active_slug) {
            let dir = profile_dir(data_dir, &profile.slug)?;
            return Ok((dir.join("lost-sheep.db"), dir.join("roads.db"), Some(profile.name.clone())));
        }
        // active points at a slug no longer in profiles (registry hand-
        // edited or corrupted) — fall through to the compatibility path
        // rather than fail to start.
    }
    Ok((data_dir.join("lost-sheep.db"), data_dir.join("roads.db"), None))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugify_basic() {
        assert_eq!(slugify("Winchester"), "winchester");
        assert_eq!(slugify("Woodstock (evening)"), "woodstock-evening");
        assert_eq!(slugify("  Front   Royal  "), "front-royal");
        assert_eq!(slugify("!!!"), "");
    }

    #[test]
    fn resolve_paths_falls_back_when_no_active_profile() {
        let data_dir = std::env::temp_dir().join(format!("lostsheep_profiles_test_{}", uuid::Uuid::new_v4().simple()));
        let registry = Registry::default();
        let (db, roads, name) = resolve_data_paths(&data_dir, &registry).unwrap();
        assert_eq!(db, data_dir.join("lost-sheep.db"));
        assert_eq!(roads, data_dir.join("roads.db"));
        assert!(name.is_none());
    }

    #[test]
    fn resolve_paths_uses_active_profile_directory() {
        let data_dir = std::env::temp_dir().join(format!("lostsheep_profiles_test_{}", uuid::Uuid::new_v4().simple()));
        let registry = Registry {
            profiles: vec![Profile {
                slug: "winchester".to_string(),
                name: "Winchester".to_string(),
                created_at: "2026-09-21T00:00:00Z".to_string(),
            }],
            active: Some("winchester".to_string()),
        };
        let (db, roads, name) = resolve_data_paths(&data_dir, &registry).unwrap();
        assert_eq!(db, data_dir.join("profiles").join("winchester").join("lost-sheep.db"));
        assert_eq!(roads, data_dir.join("profiles").join("winchester").join("roads.db"));
        assert_eq!(name.as_deref(), Some("Winchester"));
        std::fs::remove_dir_all(&data_dir).ok();
    }
}
