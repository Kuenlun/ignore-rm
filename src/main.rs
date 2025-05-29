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

use ignore_rm::{
    PickerError, RepositoryExt, collect_ignored_paths, confirm_delete_files, delete_paths,
    discover_topmost, pick_folder, wait_for_enter,
};
use std::{env, path::PathBuf};

fn main() -> Result<(), PickerError> {
    // Get the current directory
    let current_dir = env::current_dir()?;
    // Locate the topmost repository from the executable path
    let repo = discover_topmost(current_dir)?;
    // Let the user pick a folder
    let selected_folder = pick_folder(&repo.working_dir())?;
    println!("Path to delete temporary files: {:?}", selected_folder);

    // Obtain all the ignored paths from that path
    let mut ignored_paths_tmp: Vec<PathBuf> = Vec::new();
    let ignored_paths = collect_ignored_paths(&repo, &selected_folder, &mut ignored_paths_tmp)?;

    if !ignored_paths.is_empty() {
        // Print a header message before listing files
        println!("Files to be deleted:");
        for path in ignored_paths {
            println!("   {}", path.display());
        }

        // Confirm deletion by user
        if confirm_delete_files() {
            delete_paths(&repo.working_dir(), ignored_paths_tmp)?;
            println!("Deletion completed.");
        } else {
            println!("Aborted; no files were deleted.");
        }
    } else {
        println!("No files found for deletion.");
    }

    // Pause before exiting
    wait_for_enter()?;
    Ok(())
}
