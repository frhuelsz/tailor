//! End-to-end CLI tests exercising the pure (no-Docker, no-network) verbs against the examples.

use std::path::PathBuf;

use assert_cmd::Command;
use predicates::prelude::*;

fn examples() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../meta/docs/examples")
}

fn tailor() -> Command {
    Command::cargo_bin("tailor").unwrap()
}

#[test]
fn list_shows_workspace_images() {
    tailor()
        .arg("--manifest")
        .arg(examples().join("workspace-two-images/tailor.yaml"))
        .arg("list")
        .assert()
        .success()
        .stdout(predicates::str::contains("webserver").and(predicates::str::contains("database")));
}

#[test]
fn validate_renders_trident_cells() {
    tailor()
        .current_dir(examples().join("trident-vm-testimage"))
        .arg("validate")
        .assert()
        .success()
        .stdout(predicates::str::contains("16 cell(s) valid"));
}

#[test]
fn matrix_emits_json_for_every_cell() {
    tailor()
        .current_dir(examples().join("trident-vm-testimage"))
        .arg("matrix")
        .assert()
        .success()
        .stdout(predicates::str::contains(
            "trident-vm-testimage_grub_amd64_3.0_base_cosi",
        ));
}

#[test]
fn matrix_format_slugs_prints_one_bare_slug_per_line() {
    let assert = tailor()
        .current_dir(examples().join("trident-vm-testimage"))
        .args(["matrix", "--format", "slugs"])
        .assert()
        .success();
    let out = String::from_utf8_lossy(&assert.get_output().stdout);
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines.len(), 16, "one slug per cell");
    assert!(lines.contains(&"trident-vm-testimage_grub_amd64_3.0_base_cosi"));
    assert!(!out.contains('{'), "slugs format is not JSON");
}

#[test]
fn slugs_subcommand_matches_matrix_format_slugs() {
    let from_matrix = tailor()
        .current_dir(examples().join("trident-vm-testimage"))
        .args(["matrix", "--format", "slugs"])
        .assert()
        .success();
    let from_slugs = tailor()
        .current_dir(examples().join("trident-vm-testimage"))
        .arg("slugs")
        .assert()
        .success();
    assert_eq!(
        from_slugs.get_output().stdout,
        from_matrix.get_output().stdout,
        "`slugs` must match `matrix --format slugs`"
    );
}

#[test]
fn show_lists_the_dimensions_and_their_values() {
    tailor()
        .current_dir(examples().join("trident-vm-testimage"))
        .args(["show", "trident-vm-testimage"])
        .assert()
        .success()
        .stdout(
            predicates::str::contains("16 cell(s) across 4 axis(es)")
                .and(predicates::str::contains("variant"))
                .and(predicates::str::contains(
                    "grub, root-verity, usr-verity, vm-img",
                ))
                .and(predicates::str::contains("release"))
                .and(predicates::str::contains("3.0, 4.0")),
        );
}

#[test]
fn build_dry_run_prints_docker_prelude_multiline_offline() {
    tailor()
        .current_dir(examples().join("trident-vm-testimage"))
        .args(["build", "--dry-run"])
        .assert()
        .success()
        .stdout(
            predicates::str::contains("(dry-run)")
                .and(predicates::str::contains("docker run \\"))
                .and(predicates::str::contains("--privileged"))
                .and(predicates::str::contains("-v /:/host"))
                .and(predicates::str::contains("--output-image-format cosi")),
        );
}

#[test]
fn select_pins_a_single_cell() {
    tailor()
        .current_dir(examples().join("trident-vm-testimage"))
        .args([
            "build",
            "--dry-run",
            "-s",
            "variant=grub,arch=amd64,release=3.0,phase=base",
        ])
        .assert()
        .success()
        .stdout(
            predicates::str::contains("1 cell(s) (dry-run)").and(predicates::str::contains(
                "trident-vm-testimage_grub_amd64_3.0_base_cosi",
            )),
        );
}

#[test]
fn select_slice_along_one_axis() {
    // `-s arch=amd64` keeps every amd64 cell (variant[4] × release[2] × phase[1]).
    tailor()
        .current_dir(examples().join("trident-vm-testimage"))
        .args(["validate", "-s", "arch=amd64"])
        .assert()
        .success()
        .stdout(predicates::str::contains("8 cell(s) valid"));
}

#[test]
fn cell_selects_exact_slug() {
    tailor()
        .current_dir(examples().join("trident-vm-testimage"))
        .args([
            "build",
            "--dry-run",
            "--cell",
            "trident-vm-testimage_vm-img_amd64_4.0_base_vhd-fixed",
        ])
        .assert()
        .success()
        .stdout(
            predicates::str::contains("1 cell(s) (dry-run)")
                .and(predicates::str::contains("--output-image-format vhd-fixed")),
        );
}

#[test]
fn unknown_select_axis_is_rejected() {
    tailor()
        .current_dir(examples().join("trident-vm-testimage"))
        .args(["validate", "-s", "distro=fedora"])
        .assert()
        .failure()
        .stderr(predicates::str::contains("does not declare"));
}

#[test]
fn version_subcommand_matches_flag() {
    let from_flag = tailor().arg("--version").assert().success();
    let from_subcommand = tailor().arg("version").assert().success();
    assert_eq!(
        from_subcommand.get_output().stdout,
        from_flag.get_output().stdout,
        "`version` subcommand must match `--version`"
    );
}

#[test]
fn version_carries_cargo_version_and_build_metadata() {
    let assert = tailor().arg("version").assert().success();
    let out = String::from_utf8_lossy(&assert.get_output().stdout);
    assert!(out.starts_with("tailor "), "got {out:?}");
    assert!(out.contains(env!("CARGO_PKG_VERSION")), "got {out:?}");
    // SemVer build metadata is appended after a `+`.
    assert!(
        out.contains('+'),
        "expected SemVer build metadata in {out:?}"
    );
}
