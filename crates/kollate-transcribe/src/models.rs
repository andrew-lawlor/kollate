//! Models the user has added, kept in `<library dir>/models/<id>/`.

use std::path::{Path, PathBuf};

use crate::catalog::{KnownModel, Role, catalog, identify};
use crate::{Result, Transcriber, err};

/// A model with both of its files in place.
#[derive(Debug, Clone)]
pub struct Installed {
    pub model: &'static KnownModel,
    dir: PathBuf,
}

impl Installed {
    pub fn path(&self, role: Role) -> PathBuf {
        self.dir.join(self.model.file(role).name)
    }

    pub fn load(&self, gpu: bool) -> Result<Transcriber> {
        Transcriber::load(&self.path(Role::Model), &self.path(Role::Vision), gpu)
    }
}

/// Installed models, in catalog order. Files were checked when added, so
/// here only their names and sizes are compared.
pub fn installed(models_dir: &Path) -> Vec<Installed> {
    catalog()
        .iter()
        .map(|model| Installed {
            model,
            dir: models_dir.join(model.id),
        })
        .filter(|i| {
            i.model
                .files
                .iter()
                .all(|f| std::fs::metadata(i.dir.join(f.name)).is_ok_and(|m| m.len() == f.size))
        })
        .collect()
}

/// The model to use: `preferred` if it's installed, else the recommended
/// one, else any.
pub fn choose(installed: &[Installed], preferred: Option<&str>) -> Option<Installed> {
    installed
        .iter()
        .find(|i| Some(i.model.id) == preferred)
        .or_else(|| installed.iter().find(|i| i.model.recommended))
        .or_else(|| installed.first())
        .cloned()
}

/// What adding files did.
#[derive(Debug, Default)]
pub struct Added {
    /// Models that now have both files.
    pub complete: Vec<&'static KnownModel>,
    /// Models still missing a file, with the file they need.
    pub waiting: Vec<(&'static KnownModel, Role)>,
    /// Files that aren't one of the offered models (or are damaged).
    pub unknown: Vec<PathBuf>,
}

/// Copies model files into `models_dir` after checking them. Folders are
/// searched for `.gguf` files. Slow for big files: run it off the main thread.
pub fn add(models_dir: &Path, paths: &[PathBuf]) -> Result<Added> {
    kollate_core::kobo::ensure_not_on_kobo(models_dir)?;
    let mut files = Vec::new();
    for p in paths {
        if p.is_dir() {
            for entry in std::fs::read_dir(p)?.flatten() {
                let path = entry.path();
                if path
                    .extension()
                    .is_some_and(|e| e.eq_ignore_ascii_case("gguf"))
                {
                    files.push(path);
                }
            }
        } else {
            files.push(p.clone());
        }
    }
    let mut added = Added::default();
    let mut touched = Vec::new();
    for path in files {
        let Some((model, file)) = identify(&path)? else {
            added.unknown.push(path);
            continue;
        };
        let dir = models_dir.join(model.id);
        std::fs::create_dir_all(&dir)?;
        let dest = dir.join(file.name);
        if std::fs::metadata(&dest).is_ok_and(|m| m.len() == file.size) {
            touched.push(model);
            continue; // already there
        }
        // Copy beside it, then rename: a half-copied file never looks installed.
        let part = dest.with_extension("part");
        std::fs::copy(&path, &part)?;
        std::fs::rename(&part, &dest)?;
        touched.push(model);
    }
    let now = installed(models_dir);
    for model in touched {
        if now.iter().any(|i| std::ptr::eq(i.model, model)) {
            if !added.complete.iter().any(|m| std::ptr::eq(*m, model)) {
                added.complete.push(model);
            }
        } else {
            for f in &model.files {
                if !models_dir.join(model.id).join(f.name).is_file()
                    && !added
                        .waiting
                        .iter()
                        .any(|(m, r)| std::ptr::eq(*m, model) && *r == f.role)
                {
                    added.waiting.push((model, f.role));
                }
            }
        }
    }
    Ok(added)
}

/// Deletes a model's files.
pub fn remove(models_dir: &Path, model: &KnownModel) -> Result<()> {
    let dir = models_dir.join(model.id);
    if dir.exists() {
        std::fs::remove_dir_all(&dir).map_err(err)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_unknown_files_and_lists_nothing() {
        let dir = std::env::temp_dir().join(format!("kollate-models-{}", std::process::id()));
        let src = dir.join("src");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("random.gguf"), b"nope").unwrap();
        std::fs::write(src.join("readme.txt"), b"ignored").unwrap();
        let models = dir.join("models");
        let added = add(&models, &[src.clone()]).unwrap();
        assert!(added.complete.is_empty() && added.waiting.is_empty());
        assert_eq!(added.unknown, vec![src.join("random.gguf")]);
        assert!(installed(&models).is_empty());
        assert!(choose(&installed(&models), Some("qwen3-vl-4b")).is_none());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
