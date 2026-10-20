//! Runtime-only files are excluded from publication inputs and transaction staging.
use super::*;
use std::ffi::OsStr;

#[derive(Clone, serde::Serialize)]
pub(super) struct RuntimeFiles {
    paths: Vec<PathBuf>,
    automatic: bool,
}

impl Default for RuntimeFiles {
    fn default() -> Self {
        Self {
            paths: Vec::new(),
            automatic: true,
        }
    }
}

impl RuntimeFiles {
    pub(super) fn none() -> Self {
        Self {
            paths: Vec::new(),
            automatic: false,
        }
    }
    pub(super) fn legacy_baseline(&self) -> Self {
        Self {
            paths: self.paths.clone(),
            automatic: false,
        }
    }
    pub(super) fn has_automatic(&self) -> bool {
        self.automatic
    }
    pub(super) fn scoped(&self, prefix: &Path) -> Self {
        Self {
            paths: self
                .paths
                .iter()
                .filter_map(|path| {
                    let count = prefix.components().count();
                    if path.components().count() >= count && Self::overlaps(path, prefix) {
                        Some(path.components().skip(count).collect())
                    } else {
                        None
                    }
                })
                .collect(),
            automatic: self.automatic,
        }
    }
    pub(super) fn new(paths: &[String]) -> Result<Self, AppError> {
        let mut result = Self::default();
        for value in paths {
            // Portable, exact relative file paths; no glob, ADS, drive or aliases.
            if value.is_empty()
                || value
                    .chars()
                    .any(|c| c.is_control() || "\\:*?<>\"|".contains(c))
                || value.split('/').any(|p| {
                    let stem = p.split('.').next().unwrap_or("").to_ascii_uppercase();
                    p.is_empty()
                        || p == "."
                        || p == ".."
                        || p.ends_with(['.', ' '])
                        || matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
                        || (stem.len() == 4
                            && (stem.starts_with("COM") || stem.starts_with("LPT"))
                            && matches!(stem.as_bytes()[3], b'1'..=b'9'))
                })
                || value.eq_ignore_ascii_case("SKILL.md")
            {
                return Err(AppError::InvalidInput("Invalid runtime file path".into()));
            }
            let path = PathBuf::from(value);
            if !path.is_relative() || result.paths.iter().any(|p| Self::overlaps(p, &path)) {
                return Err(AppError::InvalidInput(
                    "Overlapping runtime file paths".into(),
                ));
            }
            result.paths.push(path);
        }
        Ok(result)
    }

    pub(super) fn contains(&self, path: &Path) -> bool {
        (self.automatic
            && path
                .extension()
                .is_some_and(|p| Self::component_eq(p, OsStr::new("pyc")))
            && path
                .parent()
                .and_then(Path::file_name)
                .is_some_and(|p| Self::component_eq(p, OsStr::new("__pycache__"))))
            || self.paths.iter().any(|p| {
                p.components().count() == path.components().count() && Self::overlaps(p, path)
            })
    }

    pub(super) fn explicitly_contains(&self, path: &Path) -> bool {
        self.paths
            .iter()
            .any(|p| p.components().count() == path.components().count() && Self::overlaps(p, path))
    }

    fn component_eq(a: &OsStr, b: &OsStr) -> bool {
        #[cfg(windows)]
        {
            use std::os::windows::ffi::OsStrExt;
            #[link(name = "kernel32")]
            extern "system" {
                fn CompareStringOrdinal(
                    a: *const u16,
                    a_len: i32,
                    b: *const u16,
                    b_len: i32,
                    ignore_case: i32,
                ) -> i32;
            }
            let a: Vec<_> = a.encode_wide().collect();
            let b: Vec<_> = b.encode_wide().collect();
            unsafe {
                CompareStringOrdinal(a.as_ptr(), a.len() as i32, b.as_ptr(), b.len() as i32, 1) == 2
            }
        }
        #[cfg(not(windows))]
        {
            a == b
        }
    }

    fn overlaps(a: &Path, b: &Path) -> bool {
        a.components().zip(b.components()).all(|(a, b)| {
            #[cfg(windows)]
            {
                use std::os::windows::ffi::OsStrExt;
                #[link(name = "kernel32")]
                extern "system" {
                    fn CompareStringOrdinal(
                        a: *const u16,
                        a_len: i32,
                        b: *const u16,
                        b_len: i32,
                        ignore_case: i32,
                    ) -> i32;
                }
                let a: Vec<_> = a.as_os_str().encode_wide().collect();
                let b: Vec<_> = b.as_os_str().encode_wide().collect();
                // Invalid comparisons cannot exclude a file from validation.
                unsafe {
                    CompareStringOrdinal(a.as_ptr(), a.len() as i32, b.as_ptr(), b.len() as i32, 1)
                        == 2
                }
            }
            #[cfg(not(windows))]
            {
                a == b
            }
        })
    }

    #[cfg(windows)]
    pub(super) fn paths_overlap(a: &Path, b: &Path) -> bool {
        Self::overlaps(a, b)
    }

    pub(super) fn validate_source(&self, root: &Path) -> Result<(), AppError> {
        fn walk(policy: &RuntimeFiles, root: &Path, path: &Path) -> Result<(), AppError> {
            let relative = path.strip_prefix(root).expect("descendant");
            if !relative.as_os_str().is_empty()
                && (policy.explicitly_contains(relative)
                    || policy
                        .paths
                        .iter()
                        .any(|p| RuntimeFiles::overlaps(p, relative) && path.is_file()))
            {
                return Err(AppError::InvalidInput(format!(
                    "Runtime file/source conflict: {}",
                    relative.display()
                )));
            }
            let meta = fs::symlink_metadata(path).map_err(|e| AppError::io(path, e))?;
            if meta.is_dir() {
                for entry in fs::read_dir(path).map_err(|e| AppError::io(path, e))? {
                    walk(
                        policy,
                        root,
                        &entry.map_err(|e| AppError::io(path, e))?.path(),
                    )?;
                }
            }
            Ok(())
        }
        walk(self, root, root)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_paths_are_exact_relative_files() {
        for path in [
            "",
            "/root",
            "../secret",
            "a/../secret",
            "a//b",
            "a\\b",
            "a:b",
            "*.json",
            "SKILL.md",
            "CON.txt",
            "a/last.",
        ] {
            assert!(RuntimeFiles::new(&[path.into()]).is_err(), "{path}");
        }
        assert!(RuntimeFiles::new(&["a".into(), "a/b".into()]).is_err());
        let runtime = RuntimeFiles::new(&["config/local.json".into()]).unwrap();
        assert!(runtime.contains(Path::new("config/local.json")));
        assert!(!runtime.contains(Path::new("config/local.json.extra")));
        assert!(runtime.contains(Path::new("scripts/__pycache__/a.pyc")));
        assert!(!runtime.contains(Path::new("scripts/a.pyc")));
        assert!(!runtime.contains(Path::new("__pycache__/keep.txt")));
        #[cfg(windows)]
        {
            assert!(runtime.contains(Path::new("scripts/__PYCACHE__/a.PYC")));
        }
    }
}
