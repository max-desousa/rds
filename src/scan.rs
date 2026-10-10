//! Directory scan that produces a size tree and per-extension totals.
//!
//! Symlinks are listed but not followed, so a link cycle cannot blow the walk.
//! Directories that cannot be read stay in the tree with size zero and increment
//! [`Scan::errors`] instead of failing the whole scan. The scan root itself must
//! be readable.

use std::collections::BTreeMap;
use std::fs::{self, DirEntry};
use std::io;
use std::path::{Path, PathBuf};

/// Nested directories stop at this depth and count as unreadable.
const MAX_DEPTH: usize = 256;

/// One file or directory in the scan tree.
///
/// Children are ordered by size, largest first, and then by name.
#[derive(Debug)]
pub(crate) struct Node {
    pub(crate) name: String,
    pub(crate) size: u64,
    pub(crate) file_count: u64,
    pub(crate) is_dir: bool,
    /// Lowercase extension without the dot. `None` for directories, symlinks,
    /// and files that have no extension.
    pub(crate) extension: Option<String>,
    pub(crate) children: Vec<Node>,
}

/// Bytes and file count for one extension, largest totals first.
#[derive(Debug)]
pub(crate) struct Extension {
    /// Lowercase extension without the dot, or `(none)`.
    pub(crate) name: String,
    pub(crate) size: u64,
    pub(crate) count: u64,
    /// Rank in the size ordering. The UI maps this onto a color palette.
    pub(crate) color_index: usize,
}

/// Result of scanning one path.
#[derive(Debug)]
pub(crate) struct Scan {
    pub(crate) root_path: PathBuf,
    pub(crate) root: Node,
    pub(crate) extensions: Vec<Extension>,
    pub(crate) errors: u64,
}

impl Scan {
    /// Walk `path` from the root. `path` is empty for the root node.
    pub(crate) fn node(&self, path: &[usize]) -> &Node {
        let mut node = &self.root;
        for &index in path {
            node = &node.children[index];
        }
        node
    }
}

/// Scan `path`. The path is canonicalized before walking.
pub(crate) fn scan(path: &Path) -> io::Result<Scan> {
    let root_path = fs::canonicalize(path)?;
    let metadata = fs::symlink_metadata(&root_path)?;
    let mut errors = 0;
    let root = if metadata.is_dir() {
        read_dir_node(&root_path, true, 0, &mut errors)?
    } else {
        file_node(display_name(&root_path), metadata.len())
    };
    let extensions = collect_extensions(&root);
    Ok(Scan {
        root_path,
        root,
        extensions,
        errors,
    })
}

fn read_dir_node(path: &Path, is_root: bool, depth: usize, errors: &mut u64) -> io::Result<Node> {
    let name = display_name(path);
    if depth >= MAX_DEPTH {
        *errors += 1;
        return Ok(empty_dir(name));
    }

    let entries = match fs::read_dir(path) {
        Ok(entries) => entries,
        Err(error) if is_root => return Err(error),
        Err(_) => {
            *errors += 1;
            return Ok(empty_dir(name));
        }
    };

    let mut children = Vec::new();
    for entry in entries {
        match entry {
            Ok(entry) => match read_entry(&entry, depth, errors) {
                Ok(child) => children.push(child),
                Err(_) => *errors += 1,
            },
            Err(_) => *errors += 1,
        }
    }
    children.sort_by(|a, b| b.size.cmp(&a.size).then_with(|| a.name.cmp(&b.name)));

    let mut size = 0u64;
    let mut file_count = 0u64;
    for child in &children {
        size = size.saturating_add(child.size);
        file_count = file_count.saturating_add(child.file_count);
    }

    Ok(Node {
        name,
        size,
        file_count,
        is_dir: true,
        extension: None,
        children,
    })
}

fn read_entry(entry: &DirEntry, depth: usize, errors: &mut u64) -> io::Result<Node> {
    let file_type = entry.file_type()?;
    let name = entry.file_name().to_string_lossy().into_owned();
    if file_type.is_symlink() {
        return Ok(Node {
            name,
            size: 0,
            file_count: 0,
            is_dir: false,
            extension: None,
            children: Vec::new(),
        });
    }
    if file_type.is_dir() {
        return read_dir_node(&entry.path(), false, depth + 1, errors);
    }
    let len = entry.metadata()?.len();
    Ok(file_node(name, len))
}

fn file_node(name: String, len: u64) -> Node {
    let extension = extension_of(&name);
    Node {
        name,
        size: len,
        file_count: 1,
        is_dir: false,
        extension,
        children: Vec::new(),
    }
}

fn empty_dir(name: String) -> Node {
    Node {
        name,
        size: 0,
        file_count: 0,
        is_dir: true,
        extension: None,
        children: Vec::new(),
    }
}

fn display_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| path.to_string_lossy().into_owned())
}

/// `foo.rs` is `rs`. `.gitignore`, `Makefile`, and `foo.` have no extension.
fn extension_of(name: &str) -> Option<String> {
    let (stem, extension) = name.rsplit_once('.')?;
    if stem.is_empty() || extension.is_empty() {
        None
    } else {
        Some(extension.to_ascii_lowercase())
    }
}

fn collect_extensions(root: &Node) -> Vec<Extension> {
    let mut totals: BTreeMap<String, (u64, u64)> = BTreeMap::new();
    accumulate_extensions(root, &mut totals);
    let mut extensions: Vec<Extension> = totals
        .into_iter()
        .map(|(name, (size, count))| Extension {
            name,
            size,
            count,
            color_index: 0,
        })
        .collect();
    extensions.sort_by(|a, b| b.size.cmp(&a.size).then_with(|| a.name.cmp(&b.name)));
    for (index, extension) in extensions.iter_mut().enumerate() {
        extension.color_index = index;
    }
    extensions
}

fn accumulate_extensions(node: &Node, totals: &mut BTreeMap<String, (u64, u64)>) {
    if !node.is_dir && node.file_count == 1 {
        let name = node
            .extension
            .clone()
            .unwrap_or_else(|| "(none)".to_string());
        let entry = totals.entry(name).or_insert((0, 0));
        entry.0 = entry.0.saturating_add(node.size);
        entry.1 = entry.1.saturating_add(1);
    }
    for child in &node.children {
        accumulate_extensions(child, totals);
    }
}

/// Format a byte count with binary units.
///
/// Values under 100 keep one decimal (`1.5 KiB`). Larger values are whole
/// numbers (`100 KiB`) so a column of width 8 can hold them.
pub(crate) fn format_size(bytes: u64) -> String {
    const UNITS: [&str; 6] = ["B", "KiB", "MiB", "GiB", "TiB", "PiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else if value >= 99.95 {
        format!("{value:.0} {}", UNITS[unit])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// Format `part / total` as a percentage with one decimal place.
pub(crate) fn format_percent(part: u64, total: u64) -> String {
    if total == 0 {
        return "0.0%".to_string();
    }
    format!("{:.1}%", (part as f64 / total as f64) * 100.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Scratch {
        path: PathBuf,
    }

    impl Scratch {
        fn new() -> Self {
            use std::sync::atomic::{AtomicU64, Ordering};
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "foo-scan-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&path).unwrap();
            Self { path }
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    #[test]
    fn sums_nested_files_and_sorts_largest_first() {
        let scratch = Scratch::new();
        fs::write(scratch.path.join("small.txt"), vec![0_u8; 10]).unwrap();
        fs::write(scratch.path.join("big.dat"), vec![0_u8; 30]).unwrap();
        let sub = scratch.path.join("sub");
        fs::create_dir(&sub).unwrap();
        fs::write(sub.join("tiny.txt"), vec![0_u8; 5]).unwrap();

        let scan = scan(&scratch.path).unwrap();
        assert_eq!(scan.root.size, 45);
        assert_eq!(scan.root.file_count, 3);
        assert_eq!(scan.errors, 0);
        let names: Vec<&str> = scan
            .root
            .children
            .iter()
            .map(|node| node.name.as_str())
            .collect();
        assert_eq!(names, ["big.dat", "small.txt", "sub"]);
        assert_eq!(scan.root.children[2].file_count, 1);
        assert_eq!(scan.root.children[2].size, 5);

        assert_eq!(scan.extensions[0].name, "dat");
        assert_eq!(scan.extensions[0].size, 30);
        assert_eq!(scan.extensions[0].count, 1);
        assert_eq!(scan.extensions[1].name, "txt");
        assert_eq!(scan.extensions[1].size, 15);
        assert_eq!(scan.extensions[1].count, 2);
        let extension_bytes: u64 = scan.extensions.iter().map(|ext| ext.size).sum();
        assert_eq!(extension_bytes, scan.root.size);
    }

    #[test]
    fn merges_extensions_without_case_and_ignores_dotfiles() {
        let scratch = Scratch::new();
        fs::write(scratch.path.join("A.RS"), b"abcd").unwrap();
        fs::write(scratch.path.join("b.rs"), b"xy").unwrap();
        fs::write(scratch.path.join(".gitignore"), b"ignored").unwrap();
        fs::write(scratch.path.join("Makefile"), b"all").unwrap();

        let scan = scan(&scratch.path).unwrap();
        assert_eq!(scan.extensions.len(), 2);
        assert_eq!(scan.extensions[0].name, "(none)");
        assert_eq!(scan.extensions[0].count, 2);
        assert_eq!(
            scan.extensions[0].size,
            b"ignored".len() as u64 + b"all".len() as u64
        );
        assert_eq!(scan.extensions[1].name, "rs");
        assert_eq!(scan.extensions[1].count, 2);
        assert_eq!(scan.extensions[1].size, 6);
    }

    #[test]
    fn scans_a_single_file() {
        let scratch = Scratch::new();
        let file = scratch.path.join("note.txt");
        fs::write(&file, b"abcdef").unwrap();
        let scan = scan(&file).unwrap();
        assert_eq!(scan.root.name, "note.txt");
        assert_eq!(scan.root.size, 6);
        assert_eq!(scan.root.file_count, 1);
        assert!(!scan.root.is_dir);
        assert_eq!(scan.extensions.len(), 1);
        assert_eq!(scan.extensions[0].name, "txt");
    }

    #[cfg(unix)]
    #[test]
    fn symlink_is_not_followed() {
        let scratch = Scratch::new();
        let real = scratch.path.join("real");
        fs::create_dir(&real).unwrap();
        fs::write(real.join("big.bin"), vec![1_u8; 1000]).unwrap();
        std::os::unix::fs::symlink(&real, scratch.path.join("link")).unwrap();

        let scan = scan(&scratch.path).unwrap();
        assert_eq!(scan.root.size, 1000);
        assert_eq!(scan.root.file_count, 1);
        let link = scan
            .root
            .children
            .iter()
            .find(|node| node.name == "link")
            .unwrap();
        assert_eq!(link.size, 0);
        assert!(link.children.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn unreadable_directory_is_counted_and_skipped() {
        use std::os::unix::fs::PermissionsExt;

        let scratch = Scratch::new();
        fs::write(scratch.path.join("visible.txt"), b"ok").unwrap();
        let secret = scratch.path.join("secret");
        fs::create_dir(&secret).unwrap();
        fs::write(secret.join("hidden.txt"), b"hidden").unwrap();
        let mut locked = fs::metadata(&secret).unwrap().permissions();
        locked.set_mode(0o000);
        fs::set_permissions(&secret, locked).unwrap();

        let scan_result = scan(&scratch.path);
        // Running as root ignores mode 000, so only assert when the directory
        // was actually unreadable. Check before restoring the mode.
        let blocked = fs::read_dir(&secret).is_err();

        let mut restored = fs::metadata(&secret).unwrap().permissions();
        restored.set_mode(0o755);
        fs::set_permissions(&secret, restored).unwrap();

        let scan = scan_result.unwrap();
        if blocked {
            assert!(scan.errors >= 1);
            let secret_node = scan
                .root
                .children
                .iter()
                .find(|node| node.name == "secret")
                .unwrap();
            assert_eq!(secret_node.size, 0);
            assert!(secret_node.children.is_empty());
            assert_eq!(scan.root.size, 2);
        }
    }

    #[test]
    fn formats_sizes_and_percents() {
        assert_eq!(format_size(0), "0 B");
        assert_eq!(format_size(1023), "1023 B");
        assert_eq!(format_size(1024), "1.0 KiB");
        assert_eq!(format_size(1536), "1.5 KiB");
        assert_eq!(format_size(1024 * 1024), "1.0 MiB");
        assert_eq!(format_size(100 * 1024), "100 KiB");
        assert_eq!(format_size(12 * 1024 + 307), "12.3 KiB");
        assert!(format_size(1023 * 1024).chars().count() <= 8);
        assert_eq!(format_percent(0, 0), "0.0%");
        assert_eq!(format_percent(1, 4), "25.0%");
        assert_eq!(format_percent(1, 3), "33.3%");
    }

    #[test]
    fn extension_of_common_names() {
        assert_eq!(extension_of("foo.rs"), Some("rs".to_string()));
        assert_eq!(extension_of("foo.RS"), Some("rs".to_string()));
        assert_eq!(extension_of("archive.tar.gz"), Some("gz".to_string()));
        assert_eq!(extension_of(".gitignore"), None);
        assert_eq!(extension_of("Makefile"), None);
        assert_eq!(extension_of("foo."), None);
    }
}
