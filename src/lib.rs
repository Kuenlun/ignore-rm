// This file is part of ignore-rm.
//
// Copyright (C) 2025  Juan Luis Leal Contreras (Kuenlun)
//
// ignore-rm is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// ignore-rm is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.
//
// You should have received a copy of the GNU General Public License
// along with ignore-rm.  If not, see <https://www.gnu.org/licenses/>.

use dialoguer::{Confirm, FuzzySelect, Input, theme::ColorfulTheme};
use git2::Repository;
use std::process::Command;
use std::{
    fs, io,
    path::{Path, PathBuf},
};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum PickerError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Git2 error: {0}")]
    Git2(#[from] git2::Error),
    #[error("Dialoguer error: {0}")]
    DialoguerError(#[from] dialoguer::Error),
    #[error("failed to delete `{0}`: {1}")]
    DeleteError(PathBuf, #[source] std::io::Error),
    #[error("failed to StripPrefixError")]
    StripPrefixError(#[from] std::path::StripPrefixError),
    #[error("path is not valid UTF-8")]
    NonUtf8Path,
}

/// Opens a folder picker starting at `start` and returns the selected relative path.
pub fn pick_folder(start: &Path) -> Result<PathBuf, PickerError> {
    // Save the starting path to compute relative paths
    let base = start.to_path_buf();
    let mut cwd = base.clone();

    loop {
        // 1) Get and sort subdirectories
        let mut subdirs: Vec<String> = fs::read_dir(&cwd)?
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().map(|ft| ft.is_dir()).unwrap_or(false))
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        subdirs.sort();

        // 2) Build the menu: [Select this folder], .. if not at base, then the subdirectories
        let mut entries = Vec::new();
        entries.push("[Select this folder]".into());
        if cwd != base {
            entries.push("..".into());
        }
        entries.extend(subdirs.iter().cloned());

        // 3) Show the fuzzy selector
        let idx = FuzzySelect::with_theme(&ColorfulTheme::default())
            .with_prompt(format!("Current folder: {}", cwd.display()))
            .default(0)
            .items(&entries)
            .interact()?;

        match entries[idx].as_str() {
            "[Select this folder]" => {
                // Return the path relative to `start`
                let relative = cwd.strip_prefix(&base)?.to_path_buf();
                return Ok(relative);
            }
            ".." => {
                // Move up one level (only if not at the base root)
                cwd = cwd.parent().unwrap().to_path_buf();
            }
            dir_name => {
                // Enter a subdirectory
                cwd = cwd.join(dir_name);
            }
        }
    }
}

/// Extension trait to get the “working directory” of a repo,
/// falling back to the `.git` directory for bare repositories.
pub trait RepositoryExt {
    /// Returns the working tree path (for a non-bare repo),
    /// or the path to the `.git` folder (for a bare repo).
    fn working_dir(&self) -> &Path;
}

impl RepositoryExt for Repository {
    fn working_dir(&self) -> &Path {
        // `workdir()` returns Option<&Path>; `path()` returns &Path
        self.workdir().unwrap_or_else(|| self.path())
    }
}

/// Discovers the topmost Git repository starting from `start`.
/// Returns an object implementing `RepositoryExt` representing the topmost repository.
pub fn discover_topmost<P: AsRef<Path>>(start: P) -> Result<Repository, PickerError> {
    // Discover the repository from the start path
    let mut repo = Repository::discover(start.as_ref())?;

    // Compute the initial working directory (or bare repo path)
    let mut workdir = repo.working_dir();

    // Walk up through parent directories until no further repository is found
    while let Some(parent) = workdir.parent() {
        if let Ok(upper) = Repository::discover(parent) {
            repo = upper;
            // Update working directory for the newly discovered repository
            workdir = repo.working_dir();
        } else {
            break;
        }
    }

    Ok(repo)
}

/// Prompt the user to confirm deletion.
///
/// Returns `true` if the user answers “yes” (y/Y), `false` otherwise.
pub fn confirm_delete_files() -> bool {
    Confirm::with_theme(&ColorfulTheme::default())
        .with_prompt("Are you sure you want to delete these files?")
        .default(false) // Default to “no”
        .interact()
        .unwrap_or(false) // On any error, treat as “no”
}

/// Deletes each path (file or directory) in the collection,
/// interpreting each `p` as relative to `base`, printing confirmation
/// for both skips and deletes, and only returning Err if *none* succeeded.
pub fn delete_paths<P, I>(base: &Path, paths: I) -> Result<(), PickerError>
where
    I: IntoIterator<Item = P>,
    P: AsRef<Path>,
{
    let mut any_deleted = false;
    let mut last_err: Option<(PathBuf, io::Error)> = None;

    for p in paths {
        // 1. Build an absolute path under `base`
        let rel = p.as_ref();
        let full = if rel.is_absolute() {
            rel.to_path_buf()
        } else {
            base.join(rel)
        };

        // 2. If it doesn’t even exist, skip it quietly
        if !full.exists() {
            println!("Skipping {} (not found)", full.display());
            continue;
        }

        // 3. Delete it
        let op = if full.is_dir() {
            fs::remove_dir_all(&full)
        } else {
            fs::remove_file(&full)
        };

        match op {
            Ok(()) => {
                println!("Deleted {}", full.display());
                any_deleted = true;
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                // catches the rare race where it disappeared just now
                println!("Skipping {} (already gone)", full.display());
            }
            Err(e) => {
                eprintln!("Failed to delete {}: {}", full.display(), e);
                last_err = Some((full.clone(), e));
            }
        }
    }

    // 4. Only fail if there was *absolutely no* successful deletion
    if any_deleted {
        Ok(())
    } else if let Some((path, err)) = last_err {
        Err(PickerError::DeleteError(path, err))
    } else {
        // no paths at all? treat it as success
        Ok(())
    }
}

/// Waits for the user to press Enter before exiting the program.
pub fn wait_for_enter() -> Result<(), PickerError> {
    Input::<String>::new()
        .with_prompt("Press Enter to exit")
        .allow_empty(true)
        .interact_text()?;
    Ok(())
}

/// Returns a Vec<PathBuf> containing each submodule’s path (relative to the repository root, as stored in .gitmodules)
fn submodule_paths(repo_path: &Path) -> Result<Vec<PathBuf>, PickerError> {
    // Open the repository at `repo_path`
    let repo = Repository::open(repo_path)?;
    // `repo.submodules()` returns a Vec<Submodule>
    let subs = repo.submodules()?;
    // Cada path de submódulo es relativo a la raíz del repo, así que lo unimos con repo_path
    let paths = subs
        .into_iter()
        .map(|sm| repo_path.join(sm.path()))
        .collect();
    Ok(paths)
}

// Collects all ignored paths (files and directories) in a Git repository and its submodules.
// - repo_path: Path to the root of the repository.
// - rel_path: Relative path within the repository to search for ignored files.
pub fn collect_ignored_paths(
    repo_path: &Path,
    rel_path: &Path,
) -> Result<Vec<PathBuf>, PickerError> {
    // If the path is empty (""), default to "."
    let rel_path = if rel_path.as_os_str().is_empty() {
        std::path::Path::new(".")
    } else {
        rel_path
    };

    let mut ignored_paths = Vec::new();

    // Get submodule paths (if any)
    let sub_paths = match submodule_paths(repo_path) {
        Ok(paths) => paths,
        Err(PickerError::Git2(ref e)) if e.code() == git2::ErrorCode::NotFound => {
            // Submodule is not initialized
            Vec::new()
        }
        Err(e) => return Err(e),
    };

    // Recursively collect ignored paths from submodules
    for sub_path in &sub_paths {
        match collect_ignored_paths(&sub_path, Path::new(".")) {
            Ok(submodule_ignored) => ignored_paths.extend(submodule_ignored),
            Err(PickerError::Git2(ref e)) if e.code() == git2::ErrorCode::NotFound => {
                // Submodule not initialized, skip
                continue;
            }
            Err(e) => return Err(e),
        }
    }

    // Convert rel_path to &str, error if not valid UTF-8
    let path_arg = rel_path.to_str().ok_or(PickerError::NonUtf8Path)?;

    // Run the git command to list ignored files and directories
    let output = Command::new("git")
        .current_dir(repo_path)
        .args(&[
            "ls-files",
            "--others",
            "--ignored",
            "--exclude-standard",
            "--directory",
            "--",
            path_arg,
        ])
        .output()?;

    if !output.status.success() {
        let stderr_text = String::from_utf8_lossy(&output.stderr).to_string();
        return Err(PickerError::Io(std::io::Error::new(
            std::io::ErrorKind::Other,
            format!("git command failed: {}", stderr_text),
        )));
    }

    // Parse the output: each line is a path relative to repo_path
    let stdout = String::from_utf8_lossy(&output.stdout);
    let list: Vec<PathBuf> = stdout.lines().map(|line| repo_path.join(line)).collect();

    // Add found paths to the result
    ignored_paths.extend(list);

    Ok(ignored_paths)
}
