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
//!
//! # Trust
//!
//! Installing a plugin copies files into the user's home directory and, in the
//! case of a shell extension, runs inside the desktop session, so a plugin is
//! as sensitive as a program. Two things keep that predictable:
//!
//! * only plugins that ship with ddcui-rs are trusted silently; user and
//!   `DDCUI_PLUGIN_PATH` plugins are announced before they are installed, and
//! * every path that comes out of a manifest is validated (see
//!   [`validate_uuid`] and [`validate_relative_dir`]), payload directories have
//!   to stay inside their plugin directory, and symlinks are refused rather
//!   than followed.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde::Deserialize;

/// File written into an extension directory that ddcui-rs installed.
const MARKER: &str = ".ddcui-rs-plugin";

/// Longest extension uuid that is accepted.
const MAX_UUID_LEN: usize = 128;

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

/// Where a plugin was found, which decides how far it is trusted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    /// Shipped with ddcui-rs.
    Bundled,
    /// Put there by the user, in `~/.local/share/ddcui-rs/plugins`.
    User,
    /// Named by `DDCUI_PLUGIN_PATH`.
    Environment,
}

impl Origin {
    /// How to describe the origin in messages.
    fn label(self) -> &'static str {
        match self {
            Origin::Bundled => "shipped with ddcui-rs",
            Origin::User => "installed by you",
            Origin::Environment => "named by DDCUI_PLUGIN_PATH",
        }
    }

    /// Whether installing this plugin without a warning is fine.
    fn is_trusted(self) -> bool {
        self == Origin::Bundled
    }
}

/// A discovered plugin and the directory it lives in.
#[derive(Debug, Clone)]
pub struct Plugin {
    /// Parsed `plugin.json`.
    pub manifest: Manifest,
    /// Directory holding the manifest.
    pub dir: PathBuf,
    /// Where the plugin was found.
    pub origin: Origin,
}

impl Plugin {
    /// The payload directory, validated to stay inside the plugin directory.
    pub fn payload(&self) -> Result<PathBuf, String> {
        validate_relative_dir(&self.manifest.files)?;

        let dir = self.dir.join(&self.manifest.files);
        if !dir.is_dir() {
            return Err(format!("missing plugin files: {}", dir.display()));
        }

        // Resolving both sides also catches a payload that is a symlink to
        // somewhere else entirely.
        let root = canonical(&self.dir)?;
        let payload = canonical(&dir)?;
        if !payload.starts_with(&root) {
            return Err(format!(
                "{} is outside the plugin directory {}",
                payload.display(),
                root.display()
            ));
        }
        Ok(payload)
    }
}

/// Resolve a path, reporting failures with the path that caused them.
fn canonical(path: &Path) -> Result<PathBuf, String> {
    path.canonicalize()
        .map_err(|err| format!("cannot resolve {}: {err}", path.display()))
}

/// Everything that was found while looking for plugins.
#[derive(Debug, Default)]
pub struct Discovery {
    /// Usable plugins, sorted by id.
    pub plugins: Vec<Plugin>,
    /// Manifests that were found but could not be used.
    pub problems: Vec<String>,
}

/// Directories that are searched for plugins, highest priority first.
pub fn search_paths() -> Vec<PathBuf> {
    search_entries().into_iter().map(|(dir, _)| dir).collect()
}

/// The same list, each directory with the trust level that belongs to it.
///
/// A directory is listed once even when two rules name it: with a per-user
/// install (`PREFIX=$HOME/.local`) the bundled plugins live in exactly the
/// directory that would otherwise be the user's own drop-in directory.
fn search_entries() -> Vec<(PathBuf, Origin)> {
    let bundled = next_to_executable();
    let mut entries = Vec::new();

    if let Some(from_env) = std::env::var_os("DDCUI_PLUGIN_PATH") {
        for dir in std::env::split_paths(&from_env) {
            push_once(&mut entries, dir, Origin::Environment);
        }
    }
    if let Some(data) = data_home() {
        let dir = data.join("ddcui-rs").join("plugins");
        let origin = user_dir_origin(&dir, &bundled);
        push_once(&mut entries, dir, origin);
    }
    for dir in bundled {
        push_once(&mut entries, dir, Origin::Bundled);
    }
    push_once(
        &mut entries,
        PathBuf::from("/usr/share/ddcui-rs/plugins"),
        Origin::Bundled,
    );
    entries
}

/// Add `dir` unless something above it already claimed that directory.
fn push_once(entries: &mut Vec<(PathBuf, Origin)>, dir: PathBuf, origin: Origin) {
    if !entries.iter().any(|(known, _)| *known == dir) {
        entries.push((dir, origin));
    }
}

/// The trust that belongs to the user's own plugin directory.
///
/// A per-user install puts the shipped plugins in that very directory, so it
/// only counts as somebody else's files when it is not one of the directories
/// that follow the executable.
fn user_dir_origin(user_dir: &Path, bundled: &[PathBuf]) -> Origin {
    if bundled.iter().any(|known| known == user_dir) {
        Origin::Bundled
    } else {
        Origin::User
    }
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
pub fn discover() -> Discovery {
    let mut found = Discovery::default();

    for (dir, origin) in search_entries() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let plugin_dir = entry.path();
            let manifest_path = plugin_dir.join("plugin.json");
            let text = match std::fs::read_to_string(&manifest_path) {
                Ok(text) => text,
                // A directory without a manifest is not a plugin; the four
                // search directories are expected to hold other things too.
                Err(_) => continue,
            };

            let manifest: Manifest = match serde_json::from_str(&text) {
                Ok(manifest) => manifest,
                Err(err) => {
                    found
                        .problems
                        .push(format!("{}: {err}", manifest_path.display()));
                    continue;
                }
            };
            if let Err(problem) = check_manifest(&manifest) {
                found
                    .problems
                    .push(format!("{}: {problem}", manifest_path.display()));
                continue;
            }
            if found
                .plugins
                .iter()
                .any(|other| other.manifest.id == manifest.id)
            {
                continue;
            }

            found.plugins.push(Plugin {
                manifest,
                dir: plugin_dir,
                origin,
            });
        }
    }

    found
        .plugins
        .sort_by(|a, b| a.manifest.id.cmp(&b.manifest.id));
    found
}

/// Check the parts of a manifest that every command depends on.
fn check_manifest(manifest: &Manifest) -> Result<(), String> {
    if manifest.id.is_empty() || !is_plain_name(&manifest.id) {
        return Err(format!("invalid plugin id: {}", manifest.id));
    }
    if manifest.name.trim().is_empty()
        || manifest.name.len() > 256
        || manifest.name.chars().any(char::is_control)
    {
        return Err("the manifest name must be 1–256 bytes without control characters".to_string());
    }
    if manifest.summary.len() > 1024 || manifest.summary.chars().any(char::is_control) {
        return Err(
            "the manifest summary must be at most 1024 bytes without control characters"
                .to_string(),
        );
    }
    match manifest.kind.as_str() {
        "gnome-shell-extension" => {
            let Some(uuid) = manifest.uuid.as_deref() else {
                return Err("a gnome-shell-extension needs a uuid".to_string());
            };
            validate_uuid(uuid)?;
        }
        other => return Err(format!("unsupported plugin kind: {other}")),
    }
    validate_relative_dir(&manifest.files)
}

/// Look up a single plugin by id.
pub fn find(id: &str) -> Option<Plugin> {
    let found = discover();
    found
        .plugins
        .into_iter()
        .find(|plugin| plugin.manifest.id == id)
}

/// Whether a plugin is already installed for this user.
pub fn is_installed(plugin: &Plugin) -> bool {
    match install_target(plugin) {
        Ok(target) => std::fs::symlink_metadata(target).is_ok(),
        Err(_) => false,
    }
}

/// Whether ddcui-rs installed what is in the target directory.
pub fn is_managed(plugin: &Plugin) -> bool {
    match install_target(plugin) {
        Ok(target) => {
            let Ok(target_meta) = std::fs::symlink_metadata(&target) else {
                return false;
            };
            if !target_meta.file_type().is_dir() {
                return false;
            }
            let marker = target.join(MARKER);
            let Ok(marker_meta) = std::fs::symlink_metadata(&marker) else {
                return false;
            };
            if !marker_meta.file_type().is_file() {
                return false;
            }
            let expected = format!("{}\n{}\n", plugin.manifest.id, plugin.manifest.name);
            let Ok(file) = std::fs::File::open(marker) else {
                return false;
            };
            let mut contents = String::new();
            std::io::Read::take(file, expected.len() as u64 + 1)
                .read_to_string(&mut contents)
                .is_ok()
                && contents == expected
        }
        Err(_) => false,
    }
}

/// Where a plugin's files end up.
pub fn install_target(plugin: &Plugin) -> Result<PathBuf, String> {
    let kind = plugin.manifest.kind.as_str();
    if kind != "gnome-shell-extension" {
        return Err(format!("unsupported plugin kind: {kind}"));
    }
    let Some(uuid) = plugin.manifest.uuid.as_deref() else {
        return Err("the manifest has no uuid".to_string());
    };
    validate_uuid(uuid)?;

    let Some(data) = data_home() else {
        return Err("no home directory to install into".to_string());
    };
    let extensions = data.join("gnome-shell").join("extensions");
    Ok(extensions.join(uuid))
}

/// Validate a GNOME Shell extension uuid.
///
/// The shell itself accepts `[A-Za-z0-9._-]+`, which already rules out path
/// separators; the extra checks here reject empty names, `.`/`..`, leading
/// dots (hidden directories) and absurd lengths, so a manifest can never name
/// a directory outside `<extensions>/<uuid>`.
fn validate_uuid(uuid: &str) -> Result<(), String> {
    if uuid.is_empty() {
        return Err("the extension uuid is empty".to_string());
    }
    if uuid.len() > MAX_UUID_LEN {
        return Err(format!(
            "the extension uuid is longer than {MAX_UUID_LEN} characters"
        ));
    }
    if uuid == "." || uuid == ".." || uuid.starts_with('.') {
        return Err(format!("invalid extension uuid: {uuid}"));
    }
    let valid = uuid
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-' | '@'));
    if !valid {
        return Err(format!("invalid extension uuid: {uuid}"));
    }
    Ok(())
}

/// Validate the `files` field: a relative directory name without surprises.
fn validate_relative_dir(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("the files field is empty".to_string());
    }
    if name.starts_with('/') || name.starts_with('\\') || name.contains('\\') {
        return Err(format!("the files field must be relative: {name}"));
    }
    for part in name.split('/') {
        if part.is_empty() || part == "." || part == ".." {
            return Err(format!("invalid files field: {name}"));
        }
    }
    Ok(())
}

/// Whether `name` is a plain identifier (no separators, no surprises).
fn is_plain_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_UUID_LEN
        && !name.starts_with('.')
        && name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-'))
}

/// Install a plugin, returning the steps that were performed.
pub fn install(plugin: &Plugin) -> Result<Vec<String>, String> {
    let Some(uuid) = plugin.manifest.uuid.clone() else {
        return Err("the manifest has no uuid".to_string());
    };
    let target = install_target(plugin)?;
    let payload = plugin.payload()?;

    let Some(extensions) = target.parent().map(Path::to_path_buf) else {
        return Err(format!("{} has no parent directory", target.display()));
    };
    std::fs::create_dir_all(&extensions)
        .map_err(|err| format!("cannot create {}: {err}", extensions.display()))?;

    // Preserve an extension installed by the user or another tool. Replacing
    // it silently could discard settings or code unrelated to this package.
    let target_exists = match std::fs::symlink_metadata(&target) {
        Ok(metadata) if !metadata.file_type().is_dir() => {
            return Err(format!(
                "refusing to replace non-directory {}",
                target.display()
            ));
        }
        Ok(_) if !is_managed(plugin) => {
            return Err(format!(
                "{} already exists and is not a ddcui-rs installation; move it aside before installing",
                target.display()
            ));
        }
        Ok(_) => true,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => false,
        Err(err) => return Err(format!("cannot inspect {}: {err}", target.display())),
    };

    // Claim a unique staging directory rather than deleting a predictable
    // path that might belong to a user or a previous interrupted install.
    static NEXT_PLUGIN_TEMP: AtomicU64 = AtomicU64::new(0);
    let staging = loop {
        let suffix = NEXT_PLUGIN_TEMP.fetch_add(1, Ordering::Relaxed);
        let staging = extensions.join(format!(
            ".{uuid}.ddcui-rs-staging.{}.{}",
            std::process::id(),
            suffix
        ));
        match std::fs::create_dir(&staging) {
            Ok(()) => break staging,
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(err) => return Err(format!("cannot create {}: {err}", staging.display())),
        }
    };
    let backup = if target_exists {
        loop {
            let suffix = NEXT_PLUGIN_TEMP.fetch_add(1, Ordering::Relaxed);
            let backup = extensions.join(format!(
                ".{uuid}.ddcui-rs-backup.{}.{}",
                std::process::id(),
                suffix
            ));
            match std::fs::create_dir(&backup) {
                Ok(()) => break Some(backup),
                Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(err) => {
                    let _ = std::fs::remove_dir_all(&staging);
                    return Err(format!("cannot reserve {}: {err}", backup.display()));
                }
            }
        }
    } else {
        None
    };

    let copied = match copy_dir(&payload, &staging) {
        Ok(copied) => copied,
        Err(err) => {
            let _ = std::fs::remove_dir_all(&staging);
            if let Some(backup) = backup.as_ref() {
                let _ = std::fs::remove_dir_all(backup);
            }
            return Err(err);
        }
    };
    let marker = format!("{}\n{}\n", plugin.manifest.id, plugin.manifest.name);
    if let Err(err) = std::fs::write(staging.join(MARKER), marker) {
        let _ = std::fs::remove_dir_all(&staging);
        if let Some(backup) = backup.as_ref() {
            let _ = std::fs::remove_dir_all(backup);
        }
        return Err(format!("cannot write the plugin marker: {err}"));
    }

    let mut steps = Vec::new();
    if let Some(backup) = backup.as_ref() {
        if let Err(err) = std::fs::rename(&target, backup) {
            let _ = std::fs::remove_dir_all(backup);
            let _ = std::fs::remove_dir_all(&staging);
            return Err(format!("cannot move {} aside: {err}", target.display()));
        }
    }

    if let Err(err) = std::fs::rename(&staging, &target) {
        // Put the previous installation back before giving up.
        let restore_error = backup.as_ref().and_then(|backup| {
            std::fs::rename(backup, &target).err().map(|restore| {
                format!(
                    "; could not restore the previous install, which remains at {}: {restore}",
                    backup.display()
                )
            })
        });
        let _ = std::fs::remove_dir_all(&staging);
        return Err(format!(
            "cannot install into {}: {err}{}",
            target.display(),
            restore_error.unwrap_or_default()
        ));
    }
    if let Some(backup) = backup.as_ref() {
        if let Err(err) = std::fs::remove_dir_all(backup) {
            steps.push(format!(
                "could not remove the old install backup at {}: {err}",
                backup.display()
            ));
        }
    }

    steps.push(format!("copied {copied} file(s) to {}", target.display()));
    steps.push(enable_extension(&uuid));
    Ok(steps)
}

/// Enable an extension, falling back to the setting the shell reads.
fn enable_extension(uuid: &str) -> String {
    // `gnome-extensions` only lists what the running shell scanned at start-up,
    // so a fresh install would answer "Extension ... does not exist"; write the
    // setting directly in that case instead of printing that at the user.
    if shell_knows(uuid) && run_tool("gnome-extensions", &["enable", uuid]).is_ok() {
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
pub fn uninstall(plugin: &Plugin) -> Result<Vec<String>, String> {
    let Some(uuid) = plugin.manifest.uuid.clone() else {
        return Err("the manifest has no uuid".to_string());
    };
    let target = install_target(plugin)?;

    let metadata = match std::fs::symlink_metadata(&target) {
        Ok(metadata) => metadata,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return Ok(vec![format!("{uuid} was not installed")]);
        }
        Err(err) => return Err(format!("cannot inspect {}: {err}", target.display())),
    };
    if !metadata.file_type().is_dir() {
        return Err(format!(
            "refusing to remove non-directory {}",
            target.display()
        ));
    }
    if !is_managed(plugin) {
        return Err(format!(
            "refusing to remove {} because it is not a ddcui-rs installation",
            target.display()
        ));
    }

    // The target is always `<extensions>/<uuid>` with a validated uuid; check
    // its name again immediately before any shell setting or filesystem change.
    let named_right = target
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name == uuid);
    if !named_right {
        return Err(format!("refusing to remove {}", target.display()));
    }

    let mut steps = Vec::new();
    if shell_knows(&uuid) && run_tool("gnome-extensions", &["disable", &uuid]).is_ok() {
        steps.push(format!("disabled {uuid}"));
    }
    if disable_setting(&uuid).is_ok() {
        steps.push(format!("removed {uuid} from the enabled extensions"));
    }

    std::fs::remove_dir_all(&target)
        .map_err(|err| format!("cannot remove {}: {err}", target.display()))?;
    steps.push(format!("removed {}", target.display()));
    Ok(steps)
}

/// Copy a directory tree, returning how many files were written.
///
/// Symlinks are refused: a plugin may not reach outside its own directory.
fn copy_dir(source: &Path, target: &Path) -> Result<usize, String> {
    std::fs::create_dir_all(target)
        .map_err(|err| format!("cannot create {}: {err}", target.display()))?;

    let entries = std::fs::read_dir(source)
        .map_err(|err| format!("cannot read {}: {err}", source.display()))?;

    let mut copied = 0;
    for entry in entries {
        let entry =
            entry.map_err(|err| format!("cannot read an entry in {}: {err}", source.display()))?;
        let from = entry.path();
        let kind = entry
            .file_type()
            .map_err(|err| format!("cannot inspect {}: {err}", from.display()))?;

        if kind.is_symlink() {
            return Err(format!(
                "refusing to install the symlink {}",
                from.display()
            ));
        }

        let to = target.join(entry.file_name());
        if kind.is_dir() {
            copied += copy_dir(&from, &to)?;
        } else if kind.is_file() {
            std::fs::copy(&from, &to)
                .map_err(|err| format!("cannot copy {}: {err}", from.display()))?;
            copied += 1;
        } else {
            return Err(format!("refusing to install {}", from.display()));
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
    validate_uuid(uuid)?;

    // Never fall back to an empty list here: that would drop the other
    // extensions the user has enabled.
    let mut enabled = enabled_list().ok_or("cannot read the enabled extensions")?;
    if enabled.iter().any(|item| item == uuid) {
        return Ok(());
    }
    enabled.push(uuid.to_string());
    set_enabled_list(&enabled)
}

/// Drop a uuid from `org.gnome.shell enabled-extensions`.
fn disable_setting(uuid: &str) -> Result<(), String> {
    validate_uuid(uuid)?;

    let enabled = enabled_list().ok_or("cannot read the enabled extensions")?;
    let mut kept: Vec<String> = Vec::new();
    for item in enabled {
        if item != uuid {
            kept.push(item);
        }
    }
    set_enabled_list(&kept)
}

/// Write the whole enabled-extensions list back.
fn set_enabled_list(items: &[String]) -> Result<(), String> {
    let value = format!("[{}]", quoted(items));
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
    single_quoted_items(&text)
}

/// Render a list the way `gsettings` expects, with GVariant escaping.
///
/// A real GVariant library would be nicer, but the values here are uuids and
/// the format is small enough to get right: backslashes and single quotes are
/// the only characters that need escaping inside the `'...'` form.
fn quoted(items: &[String]) -> String {
    let mut parts: Vec<String> = Vec::new();
    for item in items {
        parts.push(quoted_one(item));
    }
    parts.join(", ")
}

/// Wrap one item in single quotes, escaping what has to be escaped.
fn quoted_one(item: &str) -> String {
    let mut out = String::with_capacity(item.len() + 2);
    out.push('\'');
    for ch in item.chars() {
        if ch == '\\' || ch == '\'' {
            out.push('\\');
        }
        out.push(ch);
    }
    out.push('\'');
    out
}

/// Parse the string-array form printed by `gsettings get`.
///
/// This intentionally accepts only the small GVariant subset used for
/// extension UUIDs. If the output is unfamiliar, callers must leave the
/// setting untouched instead of treating a parse failure as an empty list.
fn single_quoted_items(text: &str) -> Option<Vec<String>> {
    let mut items = Vec::new();
    let chars: Vec<char> = text.chars().collect();
    let mut index = 0;
    skip_space(&chars, &mut index);
    if chars.get(index..index + 4) == Some(&['@', 'a', 's', ' ']) {
        index += 4;
        skip_space(&chars, &mut index);
    }
    if chars.get(index) != Some(&'[') {
        return None;
    }
    index += 1;
    skip_space(&chars, &mut index);
    if chars.get(index) == Some(&']') {
        index += 1;
        skip_space(&chars, &mut index);
        return (index == chars.len()).then_some(items);
    }

    loop {
        if chars.get(index) != Some(&'\'') {
            return None;
        }
        index += 1;
        let mut item = String::new();
        let mut closed = false;
        while index < chars.len() {
            match chars[index] {
                '\'' => {
                    index += 1;
                    closed = true;
                    break;
                }
                '\\' => {
                    index += 1;
                    let escaped = *chars.get(index)?;
                    if !matches!(escaped, '\\' | '\'') {
                        return None;
                    }
                    item.push(escaped);
                }
                ch => item.push(ch),
            }
            index += 1;
        }
        if !closed {
            return None;
        }
        items.push(item);
        skip_space(&chars, &mut index);
        match chars.get(index) {
            Some(',') => {
                index += 1;
                skip_space(&chars, &mut index);
            }
            Some(']') => {
                index += 1;
                skip_space(&chars, &mut index);
                return (index == chars.len()).then_some(items);
            }
            _ => return None,
        }
    }
}

fn skip_space(chars: &[char], index: &mut usize) {
    while chars.get(*index).is_some_and(|ch| ch.is_whitespace()) {
        *index += 1;
    }
}

/// A note to print after a successful install, when one is needed.
fn follow_up(plugin: &Plugin) -> Option<String> {
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
    let found = discover();

    for problem in &found.problems {
        eprintln!("ddcui-rs: ignoring a broken manifest: {problem}");
    }

    if found.plugins.is_empty() {
        println!("no plugins found; looked in:");
        for path in search_paths() {
            println!("  {}", path.display());
        }
        return 0;
    }

    println!("{} plugin(s) found:", found.plugins.len());
    for plugin in &found.plugins {
        let state = if is_managed(plugin) {
            "installed"
        } else if is_installed(plugin) {
            "present, not managed"
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
        println!(
            "      from {} ({})",
            plugin.dir.display(),
            plugin.origin.label()
        );
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

    if !plugin.origin.is_trusted() {
        println!(
            "{} was {}, not shipped with ddcui-rs:",
            plugin.manifest.id,
            plugin.origin.label()
        );
        println!("  {}", plugin.dir.display());
        println!("Installing it copies the files above into your account.");
    }

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

    fn manifest(id: &str, uuid: &str) -> Manifest {
        Manifest {
            id: id.to_string(),
            name: "Test plugin".to_string(),
            summary: String::new(),
            kind: "gnome-shell-extension".to_string(),
            uuid: Some(uuid.to_string()),
            files: "files".to_string(),
        }
    }

    /// Build a plugin directory with one file in its payload.
    fn fake_plugin(name: &str, manifest: Manifest, payload: &str) -> Plugin {
        let dir = std::env::temp_dir().join(format!("ddcui-rs-plugin-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("files")).unwrap();
        std::fs::write(dir.join("files").join("extension.js"), payload).unwrap();
        Plugin {
            manifest,
            dir,
            origin: Origin::User,
        }
    }

    #[test]
    fn manifest_defaults_are_filled_in() {
        let text = r#"{"id":"x","name":"X","kind":"gnome-shell-extension","uuid":"x@y"}"#;
        let manifest: Manifest = serde_json::from_str(text).unwrap();
        assert_eq!(manifest.id, "x");
        assert_eq!(manifest.files, "files");
        assert!(manifest.summary.is_empty());
        assert!(check_manifest(&manifest).is_ok());
    }

    #[test]
    fn the_bundled_plugin_matches_its_extension() {
        let dir = bundled().join("gnome-quick-menu");
        let text = std::fs::read_to_string(dir.join("plugin.json")).unwrap();
        let manifest: Manifest = serde_json::from_str(&text).unwrap();
        assert_eq!(manifest.id, "gnome-quick-menu");
        assert_eq!(manifest.kind, "gnome-shell-extension");
        assert!(check_manifest(&manifest).is_ok());

        let metadata = std::fs::read_to_string(dir.join("files").join("metadata.json")).unwrap();
        let metadata: serde_json::Value = serde_json::from_str(&metadata).unwrap();
        assert_eq!(metadata["uuid"].as_str(), manifest.uuid.as_deref());

        let versions = metadata["shell-version"].as_array().unwrap();
        let known = versions.iter().any(|item| item.as_str() == Some("50"));
        assert!(known, "the extension does not claim support for GNOME 50");
    }

    #[test]
    fn the_bundled_plugin_ships_its_entry_point() {
        let dir = bundled().join("gnome-quick-menu");
        let text = std::fs::read_to_string(dir.join("plugin.json")).unwrap();
        let manifest: Manifest = serde_json::from_str(&text).unwrap();
        let plugin = Plugin {
            manifest,
            dir,
            origin: Origin::Bundled,
        };

        let payload = plugin.payload().unwrap();
        assert!(payload.join("extension.js").is_file());
        assert!(payload.join("metadata.json").is_file());
    }

    #[test]
    fn traversal_and_odd_uuids_are_rejected() {
        let long = "a".repeat(MAX_UUID_LEN + 1);
        let rejected = [
            "",
            ".",
            "..",
            "../../../../etc",
            "/etc/passwd",
            ".hidden",
            "name/../other",
            "name with space",
            "name\twith\ttab",
            "quote'name",
            "back\\slash",
            long.as_str(),
        ];
        for uuid in rejected {
            assert!(validate_uuid(uuid).is_err(), "accepted {uuid:?}");
        }

        let accepted = [
            "ddcui-rs@jerinshop7-maker",
            "name@example.com",
            "NAME_1-2.3@sub.domain",
        ];
        for uuid in accepted {
            assert!(validate_uuid(uuid).is_ok(), "rejected {uuid:?}");
        }
    }

    #[test]
    fn a_traversal_uuid_never_becomes_a_target() {
        let plugin = fake_plugin("traversal", manifest("evil", "../../../../tmp/owned"), "x");
        assert!(install_target(&plugin).is_err());
        assert!(install(&plugin).is_err());
        assert!(uninstall(&plugin).is_err());
    }

    #[test]
    fn odd_payload_directories_are_rejected() {
        let rejected = ["", "/etc", "..", "../other", "a/../../b", "a//b", "a\\b"];
        for name in rejected {
            assert!(validate_relative_dir(name).is_err(), "accepted {name:?}");
        }
        for name in ["files", "payload", "a/b"] {
            assert!(validate_relative_dir(name).is_ok(), "rejected {name:?}");
        }

        let mut plugin = fake_plugin("odd-payload", manifest("odd", "odd@example.com"), "x");
        plugin.manifest.files = "../../".to_string();
        assert!(plugin.payload().is_err());
    }

    #[test]
    fn a_symlinked_payload_is_refused() {
        let plugin = fake_plugin("symlink-payload", manifest("sym", "sym@example.com"), "x");
        let outside = std::env::temp_dir().join("ddcui-rs-plugin-symlink-target");
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::remove_dir_all(plugin.dir.join("files")).unwrap();
        std::os::unix::fs::symlink(&outside, plugin.dir.join("files")).unwrap();

        assert!(plugin.payload().is_err());
    }

    #[test]
    fn symlinks_in_the_payload_are_refused() {
        let plugin = fake_plugin("symlink-inside", manifest("sym2", "sym2@example.com"), "x");
        let payload = plugin.dir.join("files");
        std::os::unix::fs::symlink("/etc/passwd", payload.join("passwd")).unwrap();

        let target = plugin.dir.join("installed");
        let error = copy_dir(&payload, &target).unwrap_err();
        assert!(error.contains("symlink"), "{error}");

        let _ = std::fs::remove_dir_all(&target);
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
        let items = single_quoted_items(text).unwrap();
        assert_eq!(items, vec!["ding@rastersoft.com", "ubuntu-dock@ubuntu.com"]);
        assert_eq!(format!("[{}]", quoted(&items)), text);

        let mut ours = items.clone();
        ours.push("ddcui-rs@jerinshop7-maker".to_string());
        let rendered = format!("[{}]", quoted(&ours));
        assert_eq!(single_quoted_items(&rendered), Some(ours));

        // Odd entries keep their contents, and the list is not lost.
        let odd = vec!["quote'here".to_string(), "back\\slash".to_string()];
        let rendered = format!("[{}]", quoted(&odd));
        assert_eq!(single_quoted_items(&rendered), Some(odd));
    }

    #[test]
    fn plugin_ids_are_plain_names() {
        assert!(is_plain_name("gnome-quick-menu"));
        assert!(!is_plain_name("../gnome"));
        assert!(!is_plain_name(""));
        assert!(!is_plain_name(".hidden"));
        assert!(!is_plain_name("with space"));
    }

    #[test]
    fn the_plugin_search_path_lists_every_directory_once() {
        let paths = search_paths();
        let mut seen: Vec<PathBuf> = Vec::new();
        for path in &paths {
            assert!(!seen.contains(path), "{} is listed twice", path.display());
            seen.push(path.clone());
        }
    }

    #[test]
    fn plugins_next_to_the_program_are_trusted() {
        let bundled = next_to_executable();
        assert!(!bundled.is_empty(), "the executable has no directory");
        let entries = search_entries();
        for dir in bundled {
            let origin = entries
                .iter()
                .find(|(known, _)| *known == dir)
                .map(|(_, origin)| *origin)
                .unwrap_or_else(|| panic!("{} is not on the search path", dir.display()));
            assert_eq!(origin, Origin::Bundled, "{}", dir.display());
        }
    }

    #[test]
    fn a_per_user_install_is_not_somebody_elses_files() {
        let dir = PathBuf::from("/home/someone/.local/share/ddcui-rs/plugins");
        let bundled = vec![
            PathBuf::from("/home/someone/.local/bin/plugins"),
            dir.clone(),
        ];
        // `PREFIX=$HOME/.local` puts the shipped plugins here, so they are
        // trusted; anywhere else this directory is the user's own drop-ins.
        assert_eq!(user_dir_origin(&dir, &bundled), Origin::Bundled);
        assert_eq!(user_dir_origin(&dir, &[]), Origin::User);
    }

    #[test]
    fn uninstalling_a_plugin_that_is_not_there_changes_nothing() {
        let plugin = fake_plugin(
            "uninstall-missing",
            manifest("gone", "gone@example.com"),
            "x",
        );
        match uninstall(&plugin) {
            // With a home directory the target is well formed and simply is
            // not there yet.
            Ok(steps) => assert!(steps.iter().any(|step| step.contains("was not installed"))),
            // Without one there is nowhere to look in the first place.
            Err(message) => assert!(message.contains("home"), "{message}"),
        }

        // Whatever happens, the source directory is left alone.
        assert!(plugin.dir.join("files").join("extension.js").is_file());
    }
}
