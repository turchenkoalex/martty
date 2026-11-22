//! Caps the size of every `src/` file so modules get split before they sprawl.
//!
//! Counts code lines only: blank lines, `//` comments and the trailing
//! `#[cfg(test)]` module don't count, and neither do `tests.rs` files.

use std::fs;
use std::path::{Path, PathBuf};

const MAX_CODE_LINES: usize = 500;

/// Files over the cap, with their size when they were listed. They may
/// shrink but not grow; lower the number after moving code out, and drop
/// the entry once the file fits.
const OVERSIZED_FILES: &[(&str, usize)] = &[("src/pager.rs", 568)];

fn code_lines(source: &str) -> usize {
    let mut lines: Vec<&str> = source.lines().collect();
    if let Some(i) = lines
        .windows(2)
        .rposition(|w| w[0].trim() == "#[cfg(test)]" && w[1].starts_with("mod tests"))
    {
        lines.truncate(i);
    }
    lines
        .iter()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty() && !l.starts_with("//"))
        .count()
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs")
            && path.file_name().is_some_and(|n| n != "tests.rs")
        {
            out.push(path);
        }
    }
}

#[test]
fn source_files_should_stay_under_size_cap() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut files = Vec::new();
    rust_files(&root.join("src"), &mut files);
    files.sort();

    let mut errors = Vec::new();
    for path in &files {
        let rel = path
            .strip_prefix(root)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/");
        let size = code_lines(&fs::read_to_string(path).unwrap_or_default());
        let cap = OVERSIZED_FILES
            .iter()
            .find(|(f, _)| *f == rel)
            .map_or(MAX_CODE_LINES, |&(_, n)| n);
        if size > cap {
            errors.push(format!("{rel}: {size} code lines, cap {cap}"));
        } else if cap > MAX_CODE_LINES && size < cap {
            errors.push(format!(
                "{rel}: shrank to {size} code lines, lower its OVERSIZED_FILES entry"
            ));
        }
    }
    for (file, _) in OVERSIZED_FILES {
        if !root.join(file).exists() {
            errors.push(format!("{file}: listed in OVERSIZED_FILES but missing"));
        }
    }
    assert!(errors.is_empty(), "{}", errors.join("\n"));
}
