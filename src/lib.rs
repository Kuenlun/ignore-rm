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

#[cfg(test)]
mod tests {
    use super::*;

    // ---------------------------------------------------------------------
    // Helpers for tests
    // ---------------------------------------------------------------------

    /// Assert that two paths are equal after canonicalization.
    fn assert_same_path(p1: &Path, p2: &Path) {
        let err_msg = "failed to canonicalize";
        let c1 = p1.canonicalize().expect(err_msg);
        let c2 = p2.canonicalize().expect(err_msg);
        assert_eq!(c1, c2);
    }

    /// Initialize a repository and assert its working_dir matches the given path
    fn init_repo_assert_path_matches(path: &Path) -> Repository {
        let repo = Repository::init(path).expect("failed to init repo");
        assert_same_path(repo.working_dir(), path);
        repo
    }

    /// Create a temporary Git repository and return the TempDir and Repository handle
    fn init_repo_in_tempdir() -> (tempfile::TempDir, Repository) {
        let temp = tempfile::tempdir().expect("failed to create tempdir");
        let repo = init_repo_assert_path_matches(temp.path());
        (temp, repo)
    }

    // ---------------------------------------------------------------------
    // `collect_ignored_paths` tests
    // ---------------------------------------------------------------------
    mod collect_ignored_paths {
        use super::*;
        use std::io::Write;

        // Creates an empty `.gitignore` file in `dir`, then asserts that Git does not ignore it.
        fn create_gitignore(repo: &Repository, dir: &Path) -> Result<PathBuf, PickerError> {
            let gitignore_path = dir.join(".gitignore");
            fs::File::create(&gitignore_path)?;

            // Ensure that Git is not already ignoring ".gitignore" itself
            let is_ignored = repo.is_path_ignored(Path::new(".gitignore"))?;
            assert!(!is_ignored, ".gitignore should not be ignored by default");

            Ok(gitignore_path)
        }

        // Creates a file named `filename` inside `dir`. Returns its full PathBuf.
        fn create_file(dir: &Path, filename: &str) -> Result<PathBuf, PickerError> {
            let full_path = dir.join(filename);
            fs::File::create(&full_path)?;
            Ok(full_path)
        }

        // Appends exactly `pattern` (plus a newline) to the given `.gitignore` file.
        fn append_to_gitignore(gitignore: &Path, pattern: &str) -> Result<(), PickerError> {
            let mut file = fs::OpenOptions::new().append(true).open(gitignore)?;
            writeln!(file, "{}", pattern)?;
            Ok(())
        }

        // Panics if `path` is not ignored by Git.
        fn assert_ignored(repo: &Repository, path: &Path) {
            let ignored = repo
                .is_path_ignored(path)
                .unwrap_or_else(|e| panic!("Git error checking `{}`: {}", path.display(), e));
            assert!(
                ignored,
                "`{}` should be ignored according to .gitignore",
                path.display()
            );
        }

        // Panics if `path` is ignored by Git.
        fn assert_not_ignored(repo: &Repository, path: &Path) {
            let ignored = repo
                .is_path_ignored(path)
                .unwrap_or_else(|e| panic!("Git error checking `{}`: {}", path.display(), e));
            assert!(
                !ignored,
                "`{}` should NOT be ignored according to .gitignore",
                path.display()
            );
        }

        // ----------------------------------------------------------------------
        // 1. No ignored-file at all -> `collect_ignored_paths` returns empty vec
        // ----------------------------------------------------------------------
        #[test]
        fn no_ignored_file_returns_empty() -> Result<(), PickerError> {
            let (temp_dir, repo) = init_repo_in_tempdir();
            let root = temp_dir.path();

            // Create a file "not_ignored.txt" at repo root
            let not_ignored = create_file(root, "not_ignored.txt")?;
            // Assert Git does not ignore it
            assert_not_ignored(&repo, &not_ignored);

            // Collect ignored paths from the entire repo ("" = root)
            let ignored_paths = collect_ignored_paths(&root, Path::new(""))?;

            // We expect zero ignored-paths
            assert!(
                ignored_paths.is_empty(),
                "Expected no ignored paths, but found some: {:?}",
                ignored_paths
            );

            Ok(())
        }

        // ------------------------------------------------------------------------
        // 2. One ignored-file -> `collect_ignored_paths` returns exactly that file
        // ------------------------------------------------------------------------
        #[test]
        fn single_ignored_file_is_detected() -> Result<(), PickerError> {
            let (temp_dir, repo) = init_repo_in_tempdir();
            let root = temp_dir.path();

            // Create a ".gitignore"
            let gitignore = create_gitignore(&repo, root)?;

            // Create "ignored.txt" and confirm it's not ignored yet
            let ignored_txt = create_file(root, "ignored.txt")?;
            assert_not_ignored(&repo, &ignored_txt);

            // Add "ignored.txt" to .gitignore -> now Git should ignore it
            append_to_gitignore(&gitignore, "ignored.txt")?;
            assert_ignored(&repo, &ignored_txt);

            // Run collect_ignored_paths over the entire repo
            let ignored_paths = collect_ignored_paths(&root, Path::new(""))?;

            // We expect exactly 1 ignored path, and it should match `ignored_txt`.
            assert_eq!(
                ignored_paths.len(),
                1,
                "Expected exactly one ignored file, got {}",
                ignored_paths.len()
            );
            assert_same_path(&ignored_paths[0], &ignored_txt);

            Ok(())
        }

        // -----------------------------------------------------------------
        // 3. When given a single file path, `collect_ignored_paths` should
        //    return at most that file, even if there are other ignored files
        //    in the same folder.
        // -----------------------------------------------------------------
        #[test]
        fn only_specified_file_is_checked_for_ignore() -> Result<(), PickerError> {
            let (temp_dir, repo) = init_repo_in_tempdir();
            let root = temp_dir.path();

            // Create a ".gitignore"
            let gitignore = create_gitignore(&repo, root)?;

            // Create two files and add both to .gitignore
            let ignored1 = create_file(root, "ignored.txt")?;
            let ignored2 = create_file(root, "ignored2.txt")?;

            // Files should not be ignored yet
            assert_not_ignored(&repo, &ignored1);
            assert_not_ignored(&repo, &ignored2);

            append_to_gitignore(&gitignore, "ignored.txt")?;
            append_to_gitignore(&gitignore, "ignored2.txt")?;

            // Both should indeed be ignored
            assert_ignored(&repo, &ignored1);
            assert_ignored(&repo, &ignored2);

            // Now call collect_ignored_paths *only* on "ignored.txt"
            let result = collect_ignored_paths(&root, Path::new("ignored.txt"))?;

            // It must return exactly one path and that must be ignored1
            assert_eq!(
                result.len(),
                1,
                "Expected only `ignored.txt` to be returned, even though ignored2.txt also exists"
            );
            assert_same_path(&result[0], &ignored1);

            Ok(())
        }

        // ----------------------------------------------------------------------
        // 4. Ignored directory -> only the ignored directory itself should be collected,
        //    not its internal files.
        // ----------------------------------------------------------------------
        #[test]
        fn ignored_directory_and_contents_are_collected() -> Result<(), PickerError> {
            let (temp_dir, repo) = init_repo_in_tempdir();
            let root = temp_dir.path();

            // Create a ".gitignore"
            let gitignore = create_gitignore(&repo, root)?;

            // Create an ignored directory "ignored_dir" with a file inside
            let ignored_dir = root.join("ignored_dir");
            fs::create_dir(&ignored_dir)?;
            let ignored_file = create_file(&ignored_dir, "file.txt")?;
            assert_not_ignored(&repo, &ignored_file);
            assert_not_ignored(&repo, &ignored_dir);

            // Add the directory to .gitignore
            append_to_gitignore(&gitignore, "ignored_dir/")?;
            assert_ignored(&repo, &ignored_file);
            assert_ignored(&repo, &ignored_dir);

            // Collect ignored paths from the ignored directory
            let ignored_paths = collect_ignored_paths(&root, Path::new("ignored_dir"))?;

            // The only collected path should be the directory
            assert_eq!(ignored_paths.len(), 1);
            assert_same_path(&ignored_paths[0], &ignored_dir);

            Ok(())
        }
    }

    // ---------------------------------------------------------------------
    // `discover_topmost` tests
    // ---------------------------------------------------------------------
    mod discover_topmost {
        use super::*;
        use tempfile::tempdir;

        /// Assert that `discover_topmost(path)` returns the expected repository.
        fn assert_finds_topmost(path: &Path, expected: &Repository) {
            let found = discover_topmost(path).expect("discover_topmost failed");

            // Compare working directories
            assert_same_path(found.working_dir(), expected.working_dir());

            // Compare repository paths
            assert_same_path(found.path(), expected.path());
        }

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
        fn discover_topmost_error_when_no_repo() -> Result<(), PickerError> {
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
