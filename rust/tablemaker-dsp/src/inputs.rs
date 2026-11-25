//! Gathering recordings: any mix of files and directories (searched
//! recursively for WAV files).

use std::io;
use std::path::{Path, PathBuf};

fn is_wav(p: &Path) -> bool {
    p.extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("wav") || e.eq_ignore_ascii_case("wave"))
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) -> io::Result<()> {
    let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .collect();
    entries.sort();
    for p in entries {
        let hidden = p
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.starts_with('.'));
        if hidden {
            continue;
        }
        if p.is_dir() {
            walk(&p, out)?;
        } else if is_wav(&p) {
            out.push(p);
        }
    }
    Ok(())
}

/// Every WAV file named or inside a named directory, in a stable order,
/// each once.
pub fn collect(paths: &[PathBuf]) -> io::Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    for p in paths {
        if p.is_dir() {
            walk(p, &mut out)?;
        } else if p.exists() {
            out.push(p.clone());
        } else {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("{}: no such file or directory", p.display()),
            ));
        }
    }
    let mut seen = std::collections::HashSet::new();
    out.retain(|p| seen.insert(p.canonicalize().unwrap_or_else(|_| p.clone())));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_wavs_in_nested_directories() {
        let root = std::env::temp_dir().join(format!("tablemaker-inputs-{}", std::process::id()));
        let sub = root.join("b").join("deeper");
        std::fs::create_dir_all(&sub).unwrap();
        for f in [
            root.join("a.wav"),
            root.join("notes.txt"),
            sub.join("c.WAV"),
            root.join(".hidden.wav"),
        ] {
            std::fs::write(f, b"x").unwrap();
        }
        let found = collect(&[root.clone(), root.join("a.wav")]).unwrap();
        let names: Vec<_> = found
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, vec!["a.wav", "c.WAV"]);
        assert!(collect(&[root.join("missing")]).is_err());
        std::fs::remove_dir_all(&root).unwrap();
    }
}
