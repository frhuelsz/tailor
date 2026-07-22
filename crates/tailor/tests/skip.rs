//! Integration tests for image-level `skip:` — excluding an image from bulk selection so the
//! pipeline never grabs it, while still allowing an explicit build. No Docker or network.

use std::{fs, path::Path};

use assert_cmd::Command;
use predicates::prelude::*;
use tempfile::TempDir;

fn tailor() -> Command {
    Command::cargo_bin("tailor").unwrap()
}

fn tailor_in(dir: &Path) -> Command {
    let mut cmd = tailor();
    cmd.current_dir(dir);
    cmd
}

/// A scaffolded workspace (`gadget`, 4 cells) plus an experimental `exp` image marked `skip: true`.
fn workspace_with_skip() -> TempDir {
    let tmp = TempDir::new().unwrap();
    tailor_in(tmp.path())
        .args(["init", "gadget", "advanced"])
        .assert()
        .success();
    let exp = tmp.path().join("exp");
    fs::create_dir_all(&exp).unwrap();
    fs::write(
        exp.join("image.yaml"),
        "name: exp\nskip: true\nbase: { path: ./base.raw }\noutputs: [{ format: cosi }]\nconfig: {}\n",
    )
    .unwrap();
    tmp
}

#[test]
fn bulk_selection_skips_a_skip_image() {
    let ws = workspace_with_skip();
    // No image named ⇒ bulk selection ⇒ the skip image is omitted, the normal one is present.
    tailor_in(ws.path())
        .arg("slugs")
        .assert()
        .success()
        .stdout(predicate::str::contains("gadget_").and(predicate::str::contains("exp_").not()));
}

#[test]
fn naming_a_skip_image_builds_it() {
    let ws = workspace_with_skip();
    // Explicitly naming the skip image overrides the skip.
    tailor_in(ws.path())
        .args(["slugs", "exp"])
        .assert()
        .success()
        .stdout(predicate::str::contains("exp_"));
}

#[test]
fn list_marks_a_skip_image() {
    let ws = workspace_with_skip();
    tailor_in(ws.path())
        .arg("list")
        .assert()
        .success()
        .stdout(predicate::str::contains("exp").and(predicate::str::contains("(skip)")));
}
