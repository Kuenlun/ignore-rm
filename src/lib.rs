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
use git2::{Repository, Status, StatusOptions};
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

pub fn collect_ignored_paths<'a>(
    repo: &Repository,
    path_to_delete: &Path,
    out: &'a mut Vec<PathBuf>,
) -> Result<&'a Vec<PathBuf>, PickerError> {
    // Detect submodules
    for sub in repo.submodules()? {
        if let Ok(sm_repo) = sub.open() {
            // If the path_to_delete is inside a submodule:
            if path_to_delete.starts_with(&sub.path()) {
                println!(
                    "Path to delete {} is inside submodule at {}",
                    path_to_delete.display(),
                    sub.path().display()
                );
                // Strip the submodule prefix to get the relative path:
                let rel = path_to_delete
                    .strip_prefix(sub.path())
                    .expect("path_to_delete always starts with sub.path()");
                // Recurse into that submodule with the relative path.
                return collect_ignored_paths(&sm_repo, rel, out);
            }
            // If the submodule itself lies within the path_to_delete:
            if sub.path().starts_with(&path_to_delete) {
                println!(
                    "Submodule at {} lies within path to delete {}",
                    sub.path().display(),
                    path_to_delete.display()
                );
                // Collect all of its ignored files.
                collect_ignored_paths(&sm_repo, Path::new(""), out)?;
            }
        }
    }

    // Configure StatusOptions to include ignored entries and recurse into untracked directories
    println!(
        "Obtaining ignored files from {}",
        repo.working_dir().join(path_to_delete).display()
    );
    let mut opts = StatusOptions::new();
    opts.include_ignored(true)
        .recurse_ignored_dirs(true)
        .recurse_untracked_dirs(true);

    if !path_to_delete.to_str().unwrap_or("").is_empty() {
        opts.disable_pathspec_match(true).pathspec(path_to_delete);
    }

    let statuses = repo.statuses(Some(&mut opts))?;
    for entry in statuses.iter() {
        if entry.status().contains(Status::IGNORED) {
            if let Some(path) = entry.path() {
                out.push(repo.working_dir().join(path));
            }
        }
    }

    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    mod discover_topmost {
        use super::*;
        use std::fs;
        use std::path::Path;
        use tempfile::tempdir;

        // ---------------------------------------------------------------------
        // Helpers for `discover_topmost` tests
        // ---------------------------------------------------------------------

        /// Assert that two paths are equal after canonicalization
        fn assert_same_path(p1: &Path, p2: &Path, context: &str) {
            let err_msg = "failed to canonicalize";
            let c1 = p1.canonicalize().expect(err_msg);
            let c2 = p2.canonicalize().expect(err_msg);
            assert_eq!(
                c1, c2,
                "{}: path mismatch\n  left:  {:?}\n  right: {:?}",
                context, c1, c2
            );
        }

        /// Initialize a repository and assert its working_dir matches the given path
        fn init_repo_assert_path_matches(path: &Path) -> Repository {
            let repo = Repository::init(path).expect("failed to init repo");
            assert_same_path(repo.working_dir(), path, "init repo working_dir");
            repo
        }

        /// Assert that `discover_topmost(path)` returns the expected repository
        fn assert_finds_topmost(path: &Path, expected: &Repository) {
            let found = discover_topmost(path).expect("discover_topmost failed");

            assert_same_path(
                found.working_dir(),
                expected.working_dir(),
                &format!("working_dir for {:?}", path),
            );

            assert_same_path(
                found.path(),
                expected.path(),
                &format!("repo path for {:?}", path),
            );
        }

        /// Create a temporary Git repository and return the TempDir and Repository handle
        fn init_repo_in_tempdir() -> (tempfile::TempDir, Repository) {
            let temp = tempfile::tempdir().expect("failed to create tempdir");
            let repo = init_repo_assert_path_matches(temp.path());
            (temp, repo)
        }

        // ---------------------------------------------------------------------
        // `discover_topmost` tests
        // ---------------------------------------------------------------------

        // Test that discovering from the repo root returns the repo itself
        #[test]
        fn discover_topmost_from_root_returns_root() -> Result<(), PickerError> {
            let (temp, top_repo) = init_repo_in_tempdir();
            let root = temp.path();

            assert_finds_topmost(&root, &top_repo);
            Ok(())
        }

        // Test that discovering from a repo subdirectory returns the repo itself
        #[test]
        fn discover_topmost_from_subdirectory_returns_root() -> Result<(), PickerError> {
            let (temp, top_repo) = init_repo_in_tempdir();
            let root = temp.path();

            // Create a nested directory inside the repo
            let sub_dir = root.join("a");
            fs::create_dir_all(&sub_dir)?;

            assert_finds_topmost(&sub_dir, &top_repo);
            Ok(())
        }

        // Test that discovering from a repo nested subdirectory returns the repo itself
        #[test]
        fn discover_topmost_from_nested_subdirectory_returns_root() -> Result<(), PickerError> {
            let (temp, top_repo) = init_repo_in_tempdir();
            let root = temp.path();

            // Create a nested directory inside the repo
            let nested_sub_dir = root.join("a").join("b");
            fs::create_dir_all(&nested_sub_dir)?;

            assert_finds_topmost(&nested_sub_dir, &top_repo);
            Ok(())
        }

        #[test]
        fn discover_topmost_from_sub_repo_returns_root() -> Result<(), PickerError> {
            let (temp, top_repo) = init_repo_in_tempdir();
            let root = temp.path();

            // Create a nested directory inside the repo
            let sub_dir = root.join("a");
            fs::create_dir_all(&sub_dir)?;

            // Init a repo at the subdirectory
            init_repo_assert_path_matches(&sub_dir);

            assert_finds_topmost(&sub_dir, &top_repo);
            Ok(())
        }

        #[test]
        fn discover_topmost_from_nested_repo_in_sub_directory_returns_root()
        -> Result<(), PickerError> {
            let (temp, top_repo) = init_repo_in_tempdir();
            let root = temp.path();

            // Create a nested directory inside the repo
            let nested_sub_dir = root.join("a").join("b");
            fs::create_dir_all(&nested_sub_dir)?;

            // Init a repo at the nested subdirectory
            init_repo_assert_path_matches(&nested_sub_dir);

            assert_finds_topmost(&nested_sub_dir, &top_repo);
            Ok(())
        }

        #[test]
        fn discover_topmost_from_nested_sub_repo_returns_root() -> Result<(), PickerError> {
            let (temp, top_repo) = init_repo_in_tempdir();
            let root = temp.path();

            // Create a subdirectory inside the repo
            let sub_dir = root.join("a");
            fs::create_dir_all(&sub_dir)?;

            // Create a nested directory inside the repo
            let nested_sub_dir = sub_dir.join("b");
            fs::create_dir_all(&nested_sub_dir)?;

            // Init a repo at the subdirectory
            init_repo_assert_path_matches(&sub_dir);
            // Init a repo at the nested subdirectory
            init_repo_assert_path_matches(&nested_sub_dir);

            assert_finds_topmost(&nested_sub_dir, &top_repo);
            Ok(())
        }

        #[test]
        fn discovers_topmost_error_when_no_repo() -> Result<(), PickerError> {
            let temp = tempdir()?;
            let root = temp.path();

            // Call the function and expect a specific Git2 error
            let Err(PickerError::Git2(e)) = discover_topmost(&root) else {
                panic!("expected Err(PickerError::Git2) with NotFound");
            };

            // Check that libgit2 correctly identifies the error class and code
            assert_eq!(
                e.class(),
                git2::ErrorClass::Repository,
                "expected libgit2 error class Repository"
            );
            assert_eq!(
                e.code(),
                git2::ErrorCode::NotFound,
                "expected libgit2 error code NotFound"
            );
            Ok(())
        }
    }
}
