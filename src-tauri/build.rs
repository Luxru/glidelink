use std::path::PathBuf;
use std::process::Command;

fn main() {
    emit_build_label();
    tauri_build::build()
}

/// Embed `<short-hash>[-dirty]` so the UI and logs can tell exactly which
/// source tree produced a binary. Falls back to "unknown" without git.
fn emit_build_label() {
    println!("cargo:rerun-if-changed=../.git/HEAD");

    let Some(commit) = git(&["rev-parse", "--short=9", "HEAD"]) else {
        println!("cargo:rustc-env=GLIDELINK_BUILD=unknown");
        return;
    };

    // Rebuild when the checked-out branch moves, not only when HEAD changes.
    if let Some(git_dir) = git(&["rev-parse", "--absolute-git-dir"]) {
        let git_dir = PathBuf::from(git_dir);
        if let Ok(head) = std::fs::read_to_string(git_dir.join("HEAD")) {
            if let Some(reference) = head.strip_prefix("ref: ") {
                let path = git_dir.join(reference.trim());
                println!("cargo:rerun-if-changed={}", path.display());
            }
        }
    }

    let dirty = git(&["status", "--porcelain"])
        .map(|status| !status.is_empty())
        .unwrap_or(false);
    let label = if dirty {
        format!("{commit}-dirty")
    } else {
        commit
    };
    println!("cargo:rustc-env=GLIDELINK_BUILD={label}");
}

fn git(args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
}
