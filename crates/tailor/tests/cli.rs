//! End-to-end CLI integration tests.
//!
//! These drive the real `tailor` binary (no Docker, no network — only the pure verbs) against the
//! synthetic fixture tree under `tests/fixtures/`. The fixtures are purpose-built test artifacts,
//! NOT real Image Customizer configs:
//!
//! - `workspace/`  — a two-image workspace (discovery, per-image toolchain override, defaults).
//! - `standalone/` — a single image with no manifest (standalone mode, built-in default toolchain).
//! - `matrix/`     — a 3-axis matrix exercising every complex render operation (fragment overlays,
//!   list append, `$set`/`$replace`/`$remove`/`$include`, parameter interpolation, base override,
//!   rpm sources, opaque IC passthrough).

use std::path::PathBuf;

use assert_cmd::Command;
use predicates::prelude::*;

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn tailor() -> Command {
    Command::cargo_bin("tailor").unwrap()
}

/// A `tailor` invocation with the working directory set to a fixture image/workspace directory.
fn in_dir(rel: &str) -> Command {
    let mut cmd = tailor();
    cmd.current_dir(fixtures().join(rel));
    cmd
}

// ───────────────────────────── workspace: discovery, toolchains, defaults ─────────────────────────

#[test]
fn list_shows_discovered_images_and_toolchains() {
    in_dir("workspace").arg("list").assert().success().stdout(
        predicate::str::contains("app")
            .and(predicate::str::contains("db"))
            .and(predicate::str::contains("default: ic-main"))
            .and(predicate::str::contains("ic-old")),
    );
}

#[test]
fn list_via_manifest_flag_from_any_directory() {
    // `--manifest` should locate the workspace without changing the working directory.
    tailor()
        .arg("--manifest")
        .arg(fixtures().join("workspace/tailor.yaml"))
        .arg("list")
        .assert()
        .success()
        .stdout(predicate::str::contains("app").and(predicate::str::contains("db")));
}

#[test]
fn defaults_inheritance_and_architecture_override_change_cell_counts() {
    // `app` inherits the amd64-only default → 1 cell; `db` widens architectures → amd64 + arm64.
    in_dir("workspace")
        .args(["validate", "app"])
        .assert()
        .success()
        .stdout(predicate::str::contains("1 cell(s) valid"));
    in_dir("workspace")
        .args(["validate", "db"])
        .assert()
        .success()
        .stdout(predicate::str::contains("2 cell(s) valid"));
}

#[test]
fn per_image_toolchain_override_selects_a_different_image_customizer() {
    // `db` pins ic-old (1.0.0); `app` uses the workspace default ic-main (2.0.0). The dry-run
    // container reference reflects each image's resolved toolchain tag.
    in_dir("workspace")
        .args(["build", "--dry-run", "db"])
        .assert()
        .success()
        .stdout(predicate::str::contains("imagecustomizer:1.0.0"));
    in_dir("workspace")
        .args(["build", "--dry-run", "app"])
        .assert()
        .success()
        .stdout(predicate::str::contains("imagecustomizer:2.0.0"));
}

// ───────────────────────────── standalone: built-in default toolchain ─────────────────────────────

#[test]
fn standalone_image_builds_without_a_manifest() {
    in_dir("standalone")
        .arg("validate")
        .assert()
        .success()
        .stdout(predicate::str::contains("1 cell(s) valid"));
}

#[test]
fn standalone_uses_the_built_in_default_image_customizer() {
    // No `toolchain:` and no `tailor.yaml` ⇒ tailor's built-in default IC image at the `latest` tag.
    in_dir("standalone")
        .args(["build", "--dry-run"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "mcr.microsoft.com/azurelinux/imagecustomizer:latest",
        ));
}

// ───────────────────────────── matrix: expansion, slugs, dimensions ───────────────────────────────

#[test]
fn matrix_validates_every_expanded_cell() {
    // edition[2] × arch[2] × channel[2] = 8 cells.
    in_dir("matrix")
        .arg("validate")
        .assert()
        .success()
        .stdout(predicate::str::contains("8 cell(s) valid"));
}

#[test]
fn matrix_emits_json_for_every_cell() {
    in_dir("matrix").arg("matrix").assert().success().stdout(
        predicate::str::contains("gizmo_lite_amd64_stable_cosi")
            .and(predicate::str::contains("\"format\": \"raw\"")),
    );
}

#[test]
fn matrix_format_slugs_prints_one_bare_slug_per_line() {
    let assert = in_dir("matrix")
        .args(["matrix", "--format", "slugs"])
        .assert()
        .success();
    let out = String::from_utf8_lossy(&assert.get_output().stdout);
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines.len(), 8, "one slug per cell");
    assert!(lines.contains(&"gizmo_pro_arm64_edge_raw"));
    assert!(!out.contains('{'), "slugs format is not JSON");
}

#[test]
fn slugs_subcommand_matches_matrix_format_slugs() {
    let from_matrix = in_dir("matrix")
        .args(["matrix", "--format", "slugs"])
        .assert()
        .success();
    let from_slugs = in_dir("matrix").arg("slugs").assert().success();
    assert_eq!(
        from_slugs.get_output().stdout,
        from_matrix.get_output().stdout,
        "`slugs` must match `matrix --format slugs`"
    );
}

#[test]
fn show_lists_the_dimensions_and_their_values() {
    in_dir("matrix")
        .args(["show", "gizmo"])
        .assert()
        .success()
        .stdout(
            predicate::str::contains("8 cell(s) across 3 axis(es)")
                .and(predicate::str::contains("edition"))
                .and(predicate::str::contains("lite, pro"))
                .and(predicate::str::contains("channel"))
                .and(predicate::str::contains("stable, edge")),
        );
}

// ───────────────────────────── matrix: dry-run rendering & the docker prelude ─────────────────────

#[test]
fn build_dry_run_prints_the_docker_prelude_offline() {
    in_dir("matrix")
        .args(["build", "--dry-run"])
        .assert()
        .success()
        .stdout(
            predicate::str::contains("(dry-run)")
                .and(predicate::str::contains("docker run \\"))
                .and(predicate::str::contains("--privileged"))
                .and(predicate::str::contains("-v /:/host")),
        );
}

#[test]
fn dry_run_replace_directive_changes_the_output_format() {
    // `pro` $replaces the inherited cosi output with raw.
    in_dir("matrix")
        .args(["build", "--dry-run", "--cell", "gizmo_pro_amd64_stable_raw"])
        .assert()
        .success()
        .stdout(
            predicate::str::contains("1 cell(s) (dry-run)")
                .and(predicate::str::contains("--output-image-format raw")),
        );
}

#[test]
fn dry_run_set_directive_overrides_the_base_with_an_oci_reference() {
    // `edge` $sets the base to an OCI reference with `linux/${arch}` interpolated and adds an
    // rpm-source; `stable` keeps the by-arch local path base.
    in_dir("matrix")
        .args(["build", "--dry-run", "--cell", "gizmo_lite_arm64_edge_cosi"])
        .assert()
        .success()
        .stdout(
            predicate::str::contains("--image oci:registry.example/gizmo/base:edge")
                .and(predicate::str::contains("--rpm-source"))
                .and(predicate::str::contains("repos/edge.repo")),
        );
    in_dir("matrix")
        .args([
            "build",
            "--dry-run",
            "--cell",
            "gizmo_lite_amd64_stable_cosi",
        ])
        .assert()
        .success()
        .stdout(
            predicate::str::contains("--image-file")
                .and(predicate::str::contains("bases/gizmo-amd64.img")),
        );
}

// ───────────────────────────── matrix: rendered config (merge + interpolation) ────────────────────

#[test]
fn explain_renders_nested_interpolation_and_removed_packages() {
    // `edge` derives bootPkg = "boot-edge-${efiArch}"; on amd64 ${efiArch}=x64 → boot-edge-x64
    // (nested interpolation). `pro` $removes the `base-extra` baseline package.
    in_dir("matrix")
        .args([
            "explain",
            "gizmo",
            "-s",
            "edition=pro,arch=amd64,channel=edge",
        ])
        .assert()
        .success()
        .stdout(
            predicate::str::contains("boot-edge-x64")
                .and(predicate::str::contains("gizmo-core"))
                .and(predicate::str::contains("base-extra").not())
                .and(predicate::str::contains("uki")), // previewFeatures passthrough
        );
}

#[test]
fn explain_resolves_includes_and_appends_lists() {
    // `pro` splices layouts/storage/pro.yaml via $include (adds a `data` partition) and appends
    // `audit=1` to the shared kernel command line; the `stable` channel pins boot-stable.
    in_dir("matrix")
        .args([
            "explain",
            "gizmo",
            "-s",
            "edition=pro,arch=amd64,channel=stable",
        ])
        .assert()
        .success()
        .stdout(
            predicate::str::contains("/var/lib/data") // from the $included pro storage layout
                .and(predicate::str::contains("audit=1")) // appended kernel arg
                .and(predicate::str::contains("boot-stable")), // stable channel param
        );
}

// ───────────────────────────── selection: slices, single cells, validation ───────────────────────

#[test]
fn select_pins_a_single_cell() {
    in_dir("matrix")
        .args([
            "build",
            "--dry-run",
            "-s",
            "edition=lite,arch=amd64,channel=stable",
        ])
        .assert()
        .success()
        .stdout(
            predicate::str::contains("1 cell(s) (dry-run)")
                .and(predicate::str::contains("gizmo_lite_amd64_stable_cosi")),
        );
}

#[test]
fn select_slice_along_one_axis() {
    // `-s arch=amd64` keeps every amd64 cell: edition[2] × channel[2] = 4.
    in_dir("matrix")
        .args(["validate", "-s", "arch=amd64"])
        .assert()
        .success()
        .stdout(predicate::str::contains("4 cell(s) valid"));
}

#[test]
fn cell_flag_selects_an_exact_slug() {
    in_dir("matrix")
        .args(["validate", "--cell", "gizmo_pro_arm64_edge_raw"])
        .assert()
        .success()
        .stdout(predicate::str::contains("1 cell(s) valid"));
}

#[test]
fn unknown_select_axis_is_rejected() {
    in_dir("matrix")
        .args(["validate", "-s", "distro=fedora"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("distro"));
}

#[test]
fn empty_selection_is_rejected() {
    // A syntactically valid axis/value that matches no cell is a hard error (catches CI typos).
    in_dir("matrix")
        .args(["validate", "-s", "edition=enterprise"])
        .assert()
        .failure();
}

// ───────────────────────────── version ────────────────────────────────────────────────────────────

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
