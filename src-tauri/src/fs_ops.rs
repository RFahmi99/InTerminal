//! Filesystem operations behind the visual file explorer. Each mutating
//! operation also reports the equivalent PowerShell command, so GUI actions
//! double as a way to learn the CLI.

use std::cmp::Ordering;
use std::fs;
use std::os::windows::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use serde::Serialize;
use tauri::AppHandle;
use tauri_plugin_opener::OpenerExt;

use crate::error::{AppError, CmdResult, ErrorCode};
use crate::shell::quote::single_quote;

const FILE_ATTRIBUTE_HIDDEN: u32 = 0x2;
const FILE_ATTRIBUTE_SYSTEM: u32 = 0x4;
const ERROR_NOT_SAME_DEVICE: i32 = 17;
/// Rendering tens of thousands of tree rows makes the explorer (and screen
/// readers walking it) unusable; the terminal is the right tool past this.
const MAX_ENTRIES: usize = 2000;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DirEntryInfo {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    pub is_hidden: bool,
    pub is_symlink: bool,
    pub size: Option<u64>,
    pub modified_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DirListing {
    pub path: String,
    pub parent: Option<String>,
    pub entries: Vec<DirEntryInfo>,
    /// Number of visible entries before truncation.
    pub total: usize,
    pub truncated: bool,
    /// Entries that couldn't be read (e.g. access denied), skipped silently in
    /// the tree but counted so the UI can mention them.
    pub unreadable: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FsOpResult {
    pub paths: Vec<String>,
    /// The PowerShell command that does the same thing.
    pub equivalent: String,
}

/// Case-insensitive comparison that orders digit runs numerically, so
/// "file2" sorts before "file10" like in File Explorer.
pub fn natural_cmp(a: &str, b: &str) -> Ordering {
    let (mut a, mut b) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (a.peek().copied(), b.peek().copied()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(x), Some(y)) if x.is_ascii_digit() && y.is_ascii_digit() => {
                let take_number = |it: &mut std::iter::Peekable<std::str::Chars>| {
                    let mut digits = String::new();
                    while let Some(c) = it.peek().copied().filter(char::is_ascii_digit) {
                        digits.push(c);
                        it.next();
                    }
                    digits
                };
                let (na, nb) = (take_number(&mut a), take_number(&mut b));
                let (ta, tb) = (na.trim_start_matches('0'), nb.trim_start_matches('0'));
                let ordering = ta.len().cmp(&tb.len()).then_with(|| ta.cmp(tb));
                if ordering != Ordering::Equal {
                    return ordering;
                }
            }
            (Some(x), Some(y)) => {
                let ordering = x.to_lowercase().cmp(y.to_lowercase());
                if ordering != Ordering::Equal {
                    return ordering;
                }
                a.next();
                b.next();
            }
        }
    }
}

fn read_entry(entry: &fs::DirEntry) -> std::io::Result<DirEntryInfo> {
    let link_meta = entry.metadata()?;
    let is_symlink = link_meta.file_type().is_symlink();
    // Follow links so a junction or symlink to a folder shows as a folder.
    let meta = if is_symlink { fs::metadata(entry.path()).unwrap_or(link_meta.clone()) } else { link_meta.clone() };
    let attributes = link_meta.file_attributes();
    Ok(DirEntryInfo {
        name: entry.file_name().to_string_lossy().into_owned(),
        path: entry.path().to_string_lossy().into_owned(),
        is_dir: meta.is_dir(),
        is_hidden: attributes & (FILE_ATTRIBUTE_HIDDEN | FILE_ATTRIBUTE_SYSTEM) != 0,
        is_symlink,
        size: meta.is_file().then(|| meta.len()),
        modified_ms: meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as u64),
    })
}

pub fn list_dir(path: &Path, show_hidden: bool) -> CmdResult<DirListing> {
    let reader = fs::read_dir(path).map_err(|e| AppError::io("open folder", path, &e))?;
    let mut entries = Vec::new();
    let mut unreadable = 0;
    for entry in reader {
        match entry.and_then(|e| read_entry(&e)) {
            Ok(info) if show_hidden || !info.is_hidden => entries.push(info),
            Ok(_) => {}
            Err(e) => {
                log::debug!("Skipping unreadable entry in {}: {e}", path.display());
                unreadable += 1;
            }
        }
    }
    entries.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then_with(|| natural_cmp(&a.name, &b.name)));
    let total = entries.len();
    entries.truncate(MAX_ENTRIES);
    Ok(DirListing {
        path: path.to_string_lossy().into_owned(),
        parent: path.parent().map(|p| p.to_string_lossy().into_owned()),
        entries,
        total,
        truncated: total > MAX_ENTRIES,
        unreadable,
    })
}

/// Rejects names Windows can't store, so the user gets a clear message
/// instead of a cryptic OS error.
pub fn validate_name(name: &str) -> CmdResult<&str> {
    let invalid = |reason: &str| Err(AppError::new(ErrorCode::InvalidInput, format!("\"{name}\" can't be used as a name: {reason}.")));
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return invalid("it is empty");
    }
    if trimmed == "." || trimmed == ".." {
        return invalid("it is reserved");
    }
    if let Some(c) = trimmed.chars().find(|c| r#"<>:"/\|?*"#.contains(*c) || c.is_control()) {
        return invalid(&format!("it contains the character {c:?}"));
    }
    if trimmed.ends_with('.') {
        return invalid("it ends with a dot");
    }
    if trimmed.chars().count() > 255 {
        return invalid("it is longer than 255 characters");
    }
    let stem = trimmed.split('.').next().unwrap_or("").to_ascii_uppercase();
    let reserved = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || ((stem.starts_with("COM") || stem.starts_with("LPT")) && stem.len() == 4 && stem.as_bytes()[3].is_ascii_digit());
    if reserved {
        return invalid("Windows reserves it for a device");
    }
    Ok(trimmed)
}

fn lower(path: &Path) -> String {
    path.to_string_lossy().trim_end_matches('\\').to_lowercase()
}

fn ensure_absent(path: &Path) -> CmdResult<()> {
    if path.exists() {
        return Err(AppError::new(ErrorCode::InvalidInput, format!("\"{}\" already exists.", path.display())));
    }
    Ok(())
}

pub fn create_dir(parent: &Path, name: &str) -> CmdResult<FsOpResult> {
    let name = validate_name(name)?;
    let target = parent.join(name);
    ensure_absent(&target)?;
    fs::create_dir(&target).map_err(|e| AppError::io("create folder", &target, &e))?;
    Ok(FsOpResult {
        equivalent: format!("New-Item -ItemType Directory -Path {}", single_quote(&target.to_string_lossy())),
        paths: vec![target.to_string_lossy().into_owned()],
    })
}

pub fn rename(path: &Path, new_name: &str) -> CmdResult<FsOpResult> {
    let new_name = validate_name(new_name)?;
    let parent = path.parent().ok_or_else(|| AppError::new(ErrorCode::InvalidInput, "A drive can't be renamed here."))?;
    let target = parent.join(new_name);
    // Renaming "a" to "A" is a case change of the same file, not a collision.
    if lower(&target) != lower(path) {
        ensure_absent(&target)?;
    }
    rename_no_replace(path, &target).map_err(|e| AppError::io("rename", path, &e))?;
    Ok(FsOpResult {
        equivalent: format!(
            "Rename-Item -LiteralPath {} -NewName {}",
            single_quote(&path.to_string_lossy()),
            single_quote(new_name)
        ),
        paths: vec![target.to_string_lossy().into_owned()],
    })
}

/// Renames without ever replacing an existing file. Unlike `std::fs::rename`
/// (which passes MOVEFILE_REPLACE_EXISTING), Windows itself refuses if the
/// target appears between our check and the move.
fn rename_no_replace(from: &Path, to: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::MoveFileExW;
    let wide = |p: &Path| p.as_os_str().encode_wide().chain([0]).collect::<Vec<u16>>();
    let (from_w, to_w) = (wide(from), wide(to));
    // SAFETY: both buffers are NUL-terminated UTF-16 paths that outlive the call.
    let ok = unsafe { MoveFileExW(from_w.as_ptr(), to_w.as_ptr(), 0) };
    if ok == 0 { Err(std::io::Error::last_os_error()) } else { Ok(()) }
}

/// Moves items into `dest_dir` on the same drive, never overwriting anything.
pub fn move_into(sources: &[PathBuf], dest_dir: &Path) -> CmdResult<FsOpResult> {
    if !dest_dir.is_dir() {
        return Err(AppError::new(ErrorCode::NotFound, format!("\"{}\" is not a folder that exists.", dest_dir.display())));
    }
    let dest_lower = lower(dest_dir);
    let mut plan = Vec::with_capacity(sources.len());
    let mut target_names = std::collections::HashSet::new();
    for source in sources {
        let name = source
            .file_name()
            .ok_or_else(|| AppError::new(ErrorCode::InvalidInput, format!("\"{}\" can't be moved.", source.display())))?;
        let source_lower = lower(source);
        if dest_lower == source_lower || dest_lower.starts_with(&format!("{source_lower}\\")) {
            return Err(AppError::new(ErrorCode::InvalidInput, format!("\"{}\" can't be moved into itself.", source.display())));
        }
        // Two items with the same name can't both land in one folder.
        if !target_names.insert(name.to_string_lossy().to_lowercase()) {
            return Err(AppError::new(
                ErrorCode::InvalidInput,
                format!("More than one selected item is called \"{}\", so they can't all be moved into the same folder.", name.to_string_lossy()),
            ));
        }
        let target = dest_dir.join(name);
        if lower(&target) == source_lower {
            continue; // Already there.
        }
        ensure_absent(&target)?;
        plan.push((source, target));
    }

    let mut moved = Vec::with_capacity(plan.len());
    for (source, target) in &plan {
        if let Err(e) = rename_no_replace(source, target) {
            let cause = if e.raw_os_error() == Some(ERROR_NOT_SAME_DEVICE) {
                format!(
                    "moving between drives isn't supported from the file tree. In the terminal, run: Move-Item -LiteralPath {} -Destination {}",
                    single_quote(&source.to_string_lossy()),
                    single_quote(&dest_dir.to_string_lossy())
                )
            } else {
                e.to_string()
            };
            let prefix = if moved.is_empty() {
                String::new()
            } else {
                format!("Moved {} of {} items. ", moved.len(), plan.len())
            };
            return Err(AppError::new(
                ErrorCode::Io,
                format!("{prefix}Couldn't move \"{}\": {cause}", source.display()),
            ));
        }
        moved.push(target.to_string_lossy().into_owned());
    }
    let literal_paths: Vec<String> = plan.iter().map(|(s, _)| single_quote(&s.to_string_lossy())).collect();
    Ok(FsOpResult {
        equivalent: format!(
            "Move-Item -LiteralPath {} -Destination {}",
            literal_paths.join(", "),
            single_quote(&dest_dir.to_string_lossy())
        ),
        paths: moved,
    })
}

pub fn send_to_recycle_bin(paths: &[PathBuf]) -> CmdResult<FsOpResult> {
    trash::delete_all(paths).map_err(|e| AppError::new(ErrorCode::Io, format!("Could not move to the Recycle Bin: {e}")))?;
    Ok(FsOpResult {
        paths: paths.iter().map(|p| p.to_string_lossy().into_owned()).collect(),
        // Remove-Item has no Recycle Bin option, so say so instead of
        // suggesting a command that would delete permanently.
        equivalent: "# Remove-Item deletes permanently; VCE sends items to the Recycle Bin instead so they can be restored.".into(),
    })
}

#[tauri::command]
pub async fn fs_list(path: String, show_hidden: bool) -> CmdResult<DirListing> {
    list_dir(Path::new(&path), show_hidden)
}

#[tauri::command]
pub async fn fs_create_dir(parent: String, name: String) -> CmdResult<FsOpResult> {
    create_dir(Path::new(&parent), &name)
}

#[tauri::command]
pub async fn fs_rename(path: String, new_name: String) -> CmdResult<FsOpResult> {
    rename(Path::new(&path), &new_name)
}

#[tauri::command]
pub async fn fs_move(paths: Vec<String>, dest_dir: String) -> CmdResult<FsOpResult> {
    let sources: Vec<PathBuf> = paths.into_iter().map(PathBuf::from).collect();
    move_into(&sources, Path::new(&dest_dir))
}

#[tauri::command]
pub async fn fs_trash(paths: Vec<String>) -> CmdResult<FsOpResult> {
    let paths: Vec<PathBuf> = paths.into_iter().map(PathBuf::from).collect();
    send_to_recycle_bin(&paths)
}

/// Opens a file with its default app.
#[tauri::command]
pub async fn fs_open(app: AppHandle, path: String) -> CmdResult<()> {
    app.opener()
        .open_path(&path, None::<&str>)
        .map_err(|e| AppError::new(ErrorCode::Io, format!("Could not open \"{path}\": {e}")))
}

/// Shows the item selected in Windows File Explorer.
#[tauri::command]
pub async fn fs_reveal(app: AppHandle, path: String) -> CmdResult<()> {
    app.opener()
        .reveal_item_in_dir(&path)
        .map_err(|e| AppError::new(ErrorCode::Io, format!("Could not show \"{path}\" in File Explorer: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn natural_order_matches_file_explorer() {
        let mut names = vec!["file10", "File2", "file1", "alpha", "file02b"];
        names.sort_by(|a, b| natural_cmp(a, b));
        assert_eq!(names, vec!["alpha", "file1", "File2", "file02b", "file10"]);
    }

    #[test]
    fn rejects_names_windows_cannot_store() {
        for bad in ["", "  ", "..", "a/b", "a:b", "what?", "CON", "com1.txt", "lpt9", "trailing."] {
            assert!(validate_name(bad).is_err(), "{bad:?} should be rejected");
        }
        for good in ["notes.txt", "COM10", "console", "My Folder", "[draft]"] {
            assert!(validate_name(good).is_ok(), "{good:?} should be accepted");
        }
    }

    #[test]
    fn lists_folders_first_and_hides_hidden_items() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("b.txt"), "x").unwrap();
        fs::create_dir(dir.path().join("z-folder")).unwrap();
        let listing = list_dir(dir.path(), false).unwrap();
        let names: Vec<&str> = listing.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["z-folder", "b.txt"]);
        assert_eq!(listing.entries[1].size, Some(1));
    }

    #[test]
    fn create_and_rename_report_equivalent_commands() {
        let dir = tempfile::tempdir().unwrap();
        let created = create_dir(dir.path(), "new folder").unwrap();
        assert!(created.equivalent.starts_with("New-Item -ItemType Directory -Path '"));
        let renamed = rename(Path::new(&created.paths[0]), "renamed").unwrap();
        assert!(dir.path().join("renamed").is_dir());
        assert!(renamed.equivalent.ends_with("-NewName 'renamed'"));
    }

    #[test]
    fn rename_refuses_to_overwrite() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("a.txt"), "a").unwrap();
        fs::write(dir.path().join("b.txt"), "b").unwrap();
        assert!(rename(&dir.path().join("a.txt"), "b.txt").is_err());
        assert_eq!(fs::read_to_string(dir.path().join("b.txt")).unwrap(), "b");
    }

    #[test]
    fn move_refuses_overwrite_and_moving_into_itself() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("folder");
        fs::create_dir(&folder).unwrap();
        fs::write(dir.path().join("x.txt"), "outer").unwrap();
        fs::write(folder.join("x.txt"), "inner").unwrap();
        assert!(move_into(&[dir.path().join("x.txt")], &folder).is_err());
        assert_eq!(fs::read_to_string(folder.join("x.txt")).unwrap(), "inner");
        let sub = folder.join("sub");
        fs::create_dir(&sub).unwrap();
        assert!(move_into(std::slice::from_ref(&folder), &sub).is_err());
        assert!(move_into(std::slice::from_ref(&folder), &folder).is_err());
    }

    #[test]
    fn move_refuses_two_items_with_the_same_name() {
        let dir = tempfile::tempdir().unwrap();
        for sub in ["src", "lib", "old"] {
            fs::create_dir(dir.path().join(sub)).unwrap();
        }
        fs::write(dir.path().join("src").join("index.js"), "src").unwrap();
        fs::write(dir.path().join("lib").join("index.js"), "lib").unwrap();
        let sources = [dir.path().join("src").join("index.js"), dir.path().join("lib").join("index.js")];
        assert!(move_into(&sources, &dir.path().join("old")).is_err());
        assert_eq!(fs::read_to_string(&sources[0]).unwrap(), "src");
        assert_eq!(fs::read_to_string(&sources[1]).unwrap(), "lib");
    }

    #[test]
    fn rename_never_replaces_even_without_the_precheck() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("a.txt"), "a").unwrap();
        fs::write(dir.path().join("b.txt"), "b").unwrap();
        assert!(rename_no_replace(&dir.path().join("a.txt"), &dir.path().join("b.txt")).is_err());
        assert_eq!(fs::read_to_string(dir.path().join("b.txt")).unwrap(), "b");
    }

    #[test]
    fn move_moves_files_on_same_drive() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("folder");
        fs::create_dir(&folder).unwrap();
        fs::write(dir.path().join("y.txt"), "y").unwrap();
        let result = move_into(&[dir.path().join("y.txt")], &folder).unwrap();
        assert!(folder.join("y.txt").is_file());
        assert!(result.equivalent.starts_with("Move-Item -LiteralPath '"));
    }
}
