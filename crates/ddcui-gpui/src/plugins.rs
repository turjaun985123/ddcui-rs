//! Plugins: small, self-describing integrations that put ddcui-rs where a
//! desktop expects to find it.
//!
//! A plugin is a directory holding a `plugin.json` manifest plus the files it
//! installs. Because everything lives in plain files, a plugin can be dropped
//! in by hand just as easily as it can be installed from the command line.
//!
//! Plugins are looked for, in this order, with the first hit for an id
//! winning:
//!
//! 1. every directory in `DDCUI_PLUGIN_PATH` (colon separated)
//! 2. `$XDG_DATA_HOME/ddcui-rs/plugins` (`~/.local/share/ddcui-rs/plugins`)
//! 3. `plugins/` next to the executable, and `../share/ddcui-rs/plugins`
//!    beside it (the tarball, AppImage and `/usr/local` layouts)
//! 4. `/usr/share/ddcui-rs/plugins`
//!
//! Today one kind of plugin is implemented: `gnome-shell-extension`, which
//! copies a GNOME Shell extension into the user's extension directory and
//! enables it, so the button shows up in the Quick Settings menu next to the
//! power off button.

use std::path::{Path, PathBuf};

use serde::Deserialize;

/// The `plugin.json` manifest shipped with every plugin.
#[derive(Debug, Clone, Deserialize)]
pub struct Manifest {
    /// Stable identifier used on the command line.
    pub id: String,
    /// Human readable name.
    pub name: String,
    /// One-line description shown by `--plugin-list`.
    #[serde(default)]
    pub summary: String,
    /// What the plugin installs into.
    pub kind: String,
    /// GNOME Shell extension uuid (for `kind = "gnome-shell-extension"`).
    #[serde(default)]
    pub uuid: Option<String>,
    /// Directory with the files to install, relative to the manifest.
    #[serde(default = "default_files")]
    pub files: String,
}

/// The directory name used when a manifest does not say.
fn default_files() -> String {
    "files".to_string()
}

/// A discovered plugin and the directory it lives in.
#[derive(Debug, Clone)]
pub struct Plugin {
    /// Parsed `plugin.json`.
    pub manifest: Manifest,
    /// Directory holding the manifest.
    pub dir: PathBuf,
}

impl Plugin {
    /// The directory the installed files are copied from.
    pub fn source(&self) -> PathBuf {
        self.dir.join(&self.manifest.files)
    }
}

/// Directories that are searched for plugins, highest priority first.
pub fn search_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();

    if let Some(from_env) = std::env::var_os("DDCUI_PLUGIN_PATH") {
        paths.extend(std::env::split_paths(&from_env));
    }
    if let Some(data) = data_home() {
        paths.push(data.join("ddcui-rs").join("plugins"));
    }
    paths.extend(next_to_executable());
    paths.push(PathBuf::from("/usr/share/ddcui-rs/plugins"));
    paths
}

/// `$XDG_DATA_HOME`, or the conventional `~/.local/share`.
fn data_home() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("XDG_DATA_HOME") {
        return Some(PathBuf::from(dir));
    }
    std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share"))
}

/// Plugin directories that follow the executable around.
fn next_to_executable() -> Vec<PathBuf> {
    let Ok(exe) = std::env::current_exe() else {
        return Vec::new();
    };
    let Ok(exe) = exe.canonicalize() else {
        return Vec::new();
    };
    let Some(bin) = exe.parent() else {
        return Vec::new();
    };

    let mut dirs = vec![bin.join("plugins")];
    if let Some(prefix) = bin.parent() {
        dirs.push(prefix.join("share").join("ddcui-rs").join("plugins"));
    }
    dirs
}

/// Every plugin on the search path, sorted by id.
pub fn discover() -> Vec<Plugin> {
    let mut plugins: Vec<Plugin> = Vec::new();

    for dir in search_paths() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path().join("plugin.json");
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            let Ok(manifest) = serde_json::from_str::<Manifest>(&text) else {
                continue;
            };
            if contains(&plugins, &manifest.id) {
                continue;
            }
            plugins.push(Plugin {
                manifest,
                dir: entry.path(),
            });
        }
    }

    plugins.sort_by(|a, b| a.manifest.id.cmp(&b.manifest.id));
    plugins
}

/// Whether a plugin with this id was already found.
fn contains(plugins: &[Plugin], id: &str) -> bool {
    plugins.iter().any(|plugin| plugin.manifest.id == id)
}

/// Look up a single plugin by id.
pub fn find(id: &str) -> Option<Plugin> {
    let plugins = discover();
    plugins.into_iter().find(|plugin| plugin.manifest.id == id)
}

/// Whether a plugin is already installed for this user.
pub fn is_installed(plugin: &Plugin) -> bool {
    let Some(target) = install_target(plugin) else {
        return false;
    };
    target.exists()
}

/// Where a plugin's files end up, if the kind is known.
pub fn install_target(plugin: &Plugin) -> Option<PathBuf> {
    if plugin.manifest.kind != "gnome-shell-extension" {
        return None;
    }
    let uuid = plugin.manifest.uuid.as_deref()?;
    let data = data_home()?;
    let extensions = data.join("gnome-shell").join("extensions");
    Some(extensions.join(uuid))
}

/// Install a plugin, returning the steps that were performed.
pub fn install(plugin: &Plugin) -> Result<Vec<String>, String> {
    match plugin.manifest.kind.as_str() {
        "gnome-shell-extension" => install_extension(plugin),
        other => Err(format!("unsupported plugin kind: {other}")),
    }
}

/// Remove an installed plugin, returning the steps that were performed.
pub fn uninstall(plugin: &Plugin) -> Result<Vec<String>, String> {
    match plugin.manifest.kind.as_str() {
        "gnome-shell-extension" => uninstall_extension(plugin),
        other => Err(format!("unsupported plugin kind: {other}")),
    }
}

/// Copy a GNOME Shell extension into place and enable it.
fn install_extension(plugin: &Plugin) -> Result<Vec<String>, String> {
    let Some(uuid) = plugin.manifest.uuid.clone() else {
        return Err("the manifest has no uuid".to_string());
    };
    let Some(target) = install_target(plugin) else {
        return Err("no home directory to install into".to_string());
    };
    let source = plugin.source();
    if !source.is_dir() {
        return Err(format!("missing plugin files: {}", source.display()));
    }

    if target.exists() {
        std::fs::remove_dir_all(&target)
            .map_err(|err| format!("cannot replace {}: {err}", target.display()))?;
    }
    let copied = copy_dir(&source, &target)?;

    let mut steps = vec![format!("copied {copied} file(s) to {}", target.display())];
    steps.push(enable_extension(&uuid));
    Ok(steps)
}

/// Enable an extension, falling back to the setting the shell reads.
fn enable_extension(uuid: &str) -> String {
    if run_tool("gnome-extensions", &["enable", uuid]).is_ok() {
        return format!("enabled {uuid}");
    }
    match enable_setting(uuid) {
        // A fresh extension is unknown to the running shell, which only scans
        // its extension directory when it starts, so writing the setting is
        // the only way to have it come up enabled after the next sign-in.
        Ok(()) => format!("enabled {uuid} (takes effect on sign-in)"),
        Err(err) => format!("could not enable {uuid}: {err}"),
    }
}

/// Disable a GNOME Shell extension and delete its files.
fn uninstall_extension(plugin: &Plugin) -> Result<Vec<String>, String> {
    let Some(uuid) = plugin.manifest.uuid.clone() else {
        return Err("the manifest has no uuid".to_string());
    };
    let Some(target) = install_target(plugin) else {
        return Err("no home directory to change".to_string());
    };

    let mut steps = Vec::new();
    if run_tool("gnome-extensions", &["disable", &uuid]).is_ok() {
        steps.push(format!("disabled {uuid}"));
    }
    if disable_setting(&uuid).is_ok() {
        steps.push(format!("removed {uuid} from the enabled extensions"));
    }
    if target.exists() {
        std::fs::remove_dir_all(&target)
            .map_err(|err| format!("cannot remove {}: {err}", target.display()))?;
        steps.push(format!("removed {}", target.display()));
    } else {
        steps.push(format!("{uuid} was not installed"));
    }
    Ok(steps)
}

/// Copy a directory tree, returning how many files were written.
fn copy_dir(source: &Path, target: &Path) -> Result<usize, String> {
    std::fs::create_dir_all(target)
        .map_err(|err| format!("cannot create {}: {err}", target.display()))?;

    let entries = std::fs::read_dir(source)
        .map_err(|err| format!("cannot read {}: {err}", source.display()))?;

    let mut copied = 0;
    for entry in entries.flatten() {
        let from = entry.path();
        let to = target.join(entry.file_name());
        if from.is_dir() {
            copied += copy_dir(&from, &to)?;
        } else {
            std::fs::copy(&from, &to)
                .map_err(|err| format!("cannot copy {}: {err}", from.display()))?;
            copied += 1;
        }
    }
    Ok(copied)
}

/// Run a helper program, reporting its exit status as an error message.
fn run_tool(program: &str, args: &[&str]) -> Result<(), String> {
    match std::process::Command::new(program).args(args).status() {
        Ok(status) if status.success() => Ok(()),
        Ok(status) => Err(format!("{program} exited with {status}")),
        Err(err) => Err(format!("cannot run {program}: {err}")),
    }
}

/// Whether the running GNOME Shell already knows this extension.
fn shell_knows(uuid: &str) -> bool {
    let mut command = std::process::Command::new("gnome-extensions");
    command.arg("list");
    let Ok(output) = command.output() else {
        return false;
    };
    if !output.status.success() {
        return false;
    }

    let text = String::from_utf8_lossy(&output.stdout);
    for line in text.lines() {
        if line.trim() == uuid {
            return true;
        }
    }
    false
}

/// Add a uuid to `org.gnome.shell enabled-extensions`, the setting the shell
/// reads at start-up (`gnome-extensions enable` writes the same one).
fn enable_setting(uuid: &str) -> Result<(), String> {
    let mut enabled = enabled_list().unwrap_or_default();
    if enabled.iter().any(|item| item == uuid) {
        return Ok(());
    }
    enabled.push(uuid.to_string());
    let value = format!("[{}]", quoted(&enabled));
    let args = ["set", "org.gnome.shell", "enabled-extensions", &value];
    run_tool("gsettings", &args)
}

/// Drop a uuid from `org.gnome.shell enabled-extensions`.
fn disable_setting(uuid: &str) -> Result<(), String> {
    let enabled = enabled_list().unwrap_or_default();
    let mut kept: Vec<String> = Vec::new();
    for item in enabled {
        if item != uuid {
            kept.push(item);
        }
    }
    let value = format!("[{}]", quoted(&kept));
    let args = ["set", "org.gnome.shell", "enabled-extensions", &value];
    run_tool("gsettings", &args)
}

/// The currently enabled extensions, read through `gsettings`.
fn enabled_list() -> Option<Vec<String>> {
    let output = std::process::Command::new("gsettings")
        .args(["get", "org.gnome.shell", "enabled-extensions"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    Some(single_quoted_items(&text))
}

/// Render a list as the `['a', 'b']` text `gsettings` expects.
fn quoted(items: &[String]) -> String {
    let mut parts: Vec<String> = Vec::new();
    for item in items {
        parts.push(format!("'{item}'"));
    }
    parts.join(", ")
}

/// Pull the single quoted strings out of `gsettings` output.
fn single_quoted_items(text: &str) -> Vec<String> {
    let mut items = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find('\'') {
        let after = &rest[start + 1..];
        let Some(end) = after.find('\'') else {
            break;
        };
        items.push(after[..end].to_string());
        rest = &after[end + 1..];
    }
    items
}

/// A note to print after a successful install, when one is needed.
fn follow_up(plugin: &Plugin) -> Option<String> {
    if plugin.manifest.kind != "gnome-shell-extension" {
        return None;
    }
    let uuid = plugin.manifest.uuid.as_deref()?;
    if shell_knows(uuid) {
        return None;
    }

    let note = "Sign out and back in once: GNOME Shell only looks for new \
                extensions when it starts. The button then shows up in the \
                Quick Settings menu (top right corner), right next to the \
                power off button.";
    Some(note.to_string())
}

/// `--plugin-list`
pub fn run_list() -> i32 {
    let plugins = discover();
    if plugins.is_empty() {
        println!("no plugins found; looked in:");
        for path in search_paths() {
            println!("  {}", path.display());
        }
        return 0;
    }

    println!("{} plugin(s) found:", plugins.len());
    for plugin in &plugins {
        let state = if is_installed(plugin) {
            "installed"
        } else {
            "not installed"
        };
        let manifest = &plugin.manifest;
        let id = &manifest.id;
        let name = &manifest.name;
        let kind = &manifest.kind;
        println!("  {id}  [{state}]  {name} ({kind})");
        if !manifest.summary.is_empty() {
            println!("      {}", manifest.summary);
        }
    }
    0
}

/// `--plugin-install <id>`
pub fn run_install(id: &str) -> i32 {
    let Some(plugin) = find(id) else {
        eprintln!("ddcui-rs: unknown plugin: {id}");
        eprintln!("Run 'ddcui-rs --plugin-list' to see what is available.");
        return 1;
    };

    match install(&plugin) {
        Ok(steps) => {
            println!("{}: {}", plugin.manifest.id, plugin.manifest.name);
            for step in steps {
                println!("  {step}");
            }
            if let Some(note) = follow_up(&plugin) {
                println!("{note}");
            }
            0
        }
        Err(message) => {
            eprintln!("ddcui-rs: cannot install {}: {message}", plugin.manifest.id);
            1
        }
    }
}

/// `--plugin-uninstall <id>`
pub fn run_uninstall(id: &str) -> i32 {
    let Some(plugin) = find(id) else {
        eprintln!("ddcui-rs: unknown plugin: {id}");
        eprintln!("Run 'ddcui-rs --plugin-list' to see what is available.");
        return 1;
    };

    match uninstall(&plugin) {
        Ok(steps) => {
            println!("{}: {}", plugin.manifest.id, plugin.manifest.name);
            for step in steps {
                println!("  {step}");
            }
            0
        }
        Err(message) => {
            eprintln!("ddcui-rs: cannot remove {}: {message}", plugin.manifest.id);
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The plugin directory that ships with the source tree.
    fn bundled() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join("packaging")
            .join("plugins")
    }

    #[test]
    fn manifest_defaults_are_filled_in() {
        let text = r#"{"id":"x","name":"X","kind":"gnome-shell-extension","uuid":"x@y"}"#;
        let manifest: Manifest = serde_json::from_str(text).unwrap();
        assert_eq!(manifest.id, "x");
        assert_eq!(manifest.files, "files");
        assert!(manifest.summary.is_empty());
    }

    #[test]
    fn the_bundled_plugin_matches_its_extension() {
        let dir = bundled().join("gnome-quick-menu");
        let text = std::fs::read_to_string(dir.join("plugin.json")).unwrap();
        let manifest: Manifest = serde_json::from_str(&text).unwrap();
        assert_eq!(manifest.id, "gnome-quick-menu");
        assert_eq!(manifest.kind, "gnome-shell-extension");

        let metadata = std::fs::read_to_string(dir.join("files").join("metadata.json")).unwrap();
        let metadata: serde_json::Value = serde_json::from_str(&metadata).unwrap();
        assert_eq!(metadata["uuid"].as_str(), manifest.uuid.as_deref());

        let versions = metadata["shell-version"].as_array().unwrap();
        let known = versions.iter().any(|item| item.as_str() == Some("50"));
        assert!(known, "the extension does not claim support for GNOME 50");
    }

    #[test]
    fn the_bundled_plugin_ships_its_entry_point() {
        let files = bundled().join("gnome-quick-menu").join("files");
        assert!(files.join("extension.js").is_file());
        assert!(files.join("metadata.json").is_file());
    }

    #[test]
    fn copy_dir_copies_a_tree() {
        let base = std::env::temp_dir().join("ddcui-rs-plugin-copy-test");
        let _ = std::fs::remove_dir_all(&base);
        let source = base.join("source");
        let target = base.join("target");
        std::fs::create_dir_all(source.join("nested")).unwrap();
        std::fs::write(source.join("a.txt"), "a").unwrap();
        std::fs::write(source.join("nested").join("b.txt"), "b").unwrap();

        let copied = copy_dir(&source, &target).unwrap();
        assert_eq!(copied, 2);
        assert!(target.join("nested").join("b.txt").is_file());

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn gsettings_lists_survive_a_round_trip() {
        let text = "['ding@rastersoft.com', 'ubuntu-dock@ubuntu.com']";
        let items = single_quoted_items(text);
        assert_eq!(items, vec!["ding@rastersoft.com", "ubuntu-dock@ubuntu.com"]);
        assert_eq!(quoted(&items), text);
    }

    #[test]
    fn targets_are_derived_from_the_manifest() {
        let manifest = Manifest {
            id: "demo".to_string(),
            name: "Demo".to_string(),
            summary: String::new(),
            kind: "gnome-shell-extension".to_string(),
            uuid: Some("demo@example.com".to_string()),
            files: "files".to_string(),
        };
        let plugin = Plugin {
            manifest,
            dir: PathBuf::from("/tmp/demo"),
        };
        assert_eq!(plugin.source(), Path::new("/tmp/demo/files"));

        let target = install_target(&plugin).unwrap();
        assert!(target.ends_with("gnome-shell/extensions/demo@example.com"));
    }
}
