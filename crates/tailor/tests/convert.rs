//! Integration tests for `tailor convert` — the workspace-free `imagecustomizer convert` wrapper.
//!
//! The dry-run path is daemon-free (it renders the container invocation via a no-op runtime), so
//! these run in CI without Docker or network. The input lives in a subdirectory of the working
//! directory so the default output dir is a safe read-write mount.

use std::{fs, path::Path};

use assert_cmd::Command;
use predicates::prelude::*;
use tempfile::TempDir;

fn tailor() -> Command {
    Command::cargo_bin("tailor").unwrap()
}

/// A temp working dir with a dummy input image at `imgs/disk.vhdx`.
fn workspace_with_input() -> (TempDir, std::path::PathBuf) {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().join("imgs");
    fs::create_dir_all(&dir).unwrap();
    let input = dir.join("disk.vhdx");
    fs::write(&input, b"not a real image, dry-run only").unwrap();
    (tmp, input)
}

fn convert_in(dir: &Path) -> Command {
    let mut cmd = tailor();
    cmd.current_dir(dir);
    cmd
}

#[test]
fn dry_run_renders_the_convert_invocation() {
    let (tmp, input) = workspace_with_input();
    convert_in(tmp.path())
        .args([
            "convert",
            input.to_str().unwrap(),
            "--to",
            "raw",
            "--dry-run",
        ])
        .assert()
        .success()
        .stdout(
            predicate::str::contains("convert")
                .and(predicate::str::contains("--image-file"))
                .and(predicate::str::contains("--output-image-format"))
                .and(predicate::str::contains("--output-image-file")),
        );
}

#[test]
fn dry_run_honors_arch_and_container_overrides() {
    let (tmp, input) = workspace_with_input();
    convert_in(tmp.path())
        .args([
            "convert",
            input.to_str().unwrap(),
            "--to",
            "vhd-fixed",
            "--arch",
            "arm64",
            "--container",
            "example.test/ic:pinned",
            "--dry-run",
        ])
        .assert()
        .success()
        .stdout(
            predicate::str::contains("--platform linux/arm64")
                .and(predicate::str::contains("example.test/ic:pinned"))
                .and(predicate::str::contains("--output-image-format vhd-fixed")),
        );
}

#[test]
fn an_unsupported_format_is_rejected() {
    let (tmp, input) = workspace_with_input();
    convert_in(tmp.path())
        .args([
            "convert",
            input.to_str().unwrap(),
            "--to",
            "iso",
            "--dry-run",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("not a convert-supported format"));
}

#[test]
fn a_missing_input_is_a_clear_error() {
    let tmp = TempDir::new().unwrap();
    convert_in(tmp.path())
        .args(["convert", "does/not/exist.vhdx", "--to", "raw", "--dry-run"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("does not exist"));
}

#[test]
fn convert_needs_no_workspace() {
    // No `tailor.yaml` anywhere: convert must still work (unlike the workspace verbs).
    let (tmp, input) = workspace_with_input();
    assert!(!tmp.path().join("tailor.yaml").exists());
    convert_in(tmp.path())
        .args([
            "convert",
            input.to_str().unwrap(),
            "--to",
            "qcow2",
            "--dry-run",
        ])
        .assert()
        .success();
}
