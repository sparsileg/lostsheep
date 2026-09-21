// commands/profiles.rs — issue #85: thin Tauri-command layer over
// crate::profiles. Kept separate from profiles.rs itself so the registry/
// path-resolution logic stays plain, testable Rust with no Tauri
// dependency.

use crate::profiles::{self, Profile};
use tauri::{AppHandle, Manager};

fn data_dir(app: &AppHandle) -> Result<std::path::PathBuf, String> {
    app.path().app_data_dir().map_err(|e| e.to_string())
}

#[tauri::command]
pub fn list_profiles(app: AppHandle) -> Result<Vec<Profile>, String> {
    let dir = data_dir(&app)?;
    Ok(profiles::load_registry(&dir).profiles)
}

#[tauri::command]
pub fn get_active_profile(app: AppHandle) -> Result<Option<Profile>, String> {
    let dir = data_dir(&app)?;
    let registry = profiles::load_registry(&dir);
    Ok(registry
        .active
        .as_ref()
        .and_then(|slug| registry.profiles.iter().find(|p| &p.slug == slug).cloned()))
}

/// Creates a new profile and immediately makes it active — matching issue
/// #85's settled flow (create implies switch). The frontend is expected to
/// call restart_app itself right after this returns, once it's shown the
/// user the new profile was created; that's a UX beat this command
/// shouldn't own.
#[tauri::command]
pub fn create_profile(app: AppHandle, name: String) -> Result<Profile, String> {
    let name = name.trim().to_string();
    if name.is_empty() {
        return Err("profile name cannot be empty".to_string());
    }
    let slug = profiles::slugify(&name);
    if slug.is_empty() {
        return Err("profile name must contain at least one letter or number".to_string());
    }

    let dir = data_dir(&app)?;
    let mut registry = profiles::load_registry(&dir);
    if registry.profiles.iter().any(|p| p.slug == slug) {
        return Err(format!("a profile named \"{name}\" already exists"));
    }

    let profile = Profile {
        slug: slug.clone(),
        name,
        created_at: chrono::Utc::now().to_rfc3339(),
    };
    // Ensure the profile's own directory exists before it's recorded as
    // active — main.rs's next launch resolves paths straight from the
    // registry with no separate "does this directory exist" check.
    profiles::profile_dir(&dir, &slug).map_err(|e| e.to_string())?;

    registry.profiles.push(profile.clone());
    registry.active = Some(slug);
    profiles::save_registry(&dir, &registry).map_err(|e| e.to_string())?;

    Ok(profile)
}

#[tauri::command]
pub fn switch_profile(app: AppHandle, slug: String) -> Result<(), String> {
    let dir = data_dir(&app)?;
    let mut registry = profiles::load_registry(&dir);
    if !registry.profiles.iter().any(|p| p.slug == slug) {
        return Err(format!("no such profile: {slug}"));
    }
    registry.active = Some(slug);
    profiles::save_registry(&dir, &registry).map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub fn restart_app(app: AppHandle) {
    app.restart();
}
