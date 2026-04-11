use std::fs;
use std::path::{Component, Path, PathBuf};

use anyhow::{bail, Context, Result};

#[derive(Debug)]
pub struct Sandbox {
    pub(crate) root: PathBuf,
}

impl Sandbox {
    pub fn new(root: impl Into<PathBuf>) -> Result<Self> {
        let root = root.into();
        fs::create_dir_all(&root)
            .with_context(|| format!("creating sandbox root {}", root.display()))?;
        let canon = fs::canonicalize(&root)
            .with_context(|| format!("canonicalizing sandbox root {}", root.display()))?;
        Ok(Self { root: canon })
    }

    /// Resolve a workspace-relative path. Refuses absolute paths, `..`
    /// traversal, and symlink escapes.
    pub fn resolve(&self, rel: &str) -> Result<PathBuf> {
        let p = Path::new(rel);
        if p.is_absolute() {
            bail!("path must be relative to the workspace");
        }
        for comp in p.components() {
            match comp {
                Component::ParentDir => bail!("path cannot contain '..'"),
                Component::Prefix(_) | Component::RootDir => bail!("invalid path"),
                _ => {}
            }
        }
        let joined = self.root.join(p);

        // Canonicalize either the target or its nearest existing ancestor to
        // defeat symlink-based escapes.
        let check_base = if joined.exists() {
            fs::canonicalize(&joined)
                .with_context(|| format!("canonicalizing {}", joined.display()))?
        } else {
            let mut cursor: &Path = joined.as_path();
            let existing = loop {
                match cursor.parent() {
                    Some(parent) if parent.exists() => break parent,
                    Some(parent) => cursor = parent,
                    None => bail!("path has no existing ancestor"),
                }
            };
            fs::canonicalize(existing)
                .with_context(|| format!("canonicalizing ancestor {}", existing.display()))?
        };

        if !check_base.starts_with(&self.root) {
            bail!("path escapes workspace");
        }
        Ok(joined)
    }
}
