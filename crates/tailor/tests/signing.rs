//! End-to-end CLI integration tests for the signing foundation (`meta/docs/signing.md`).
//!
//! These drive the real `tailor` binary against the synthetic `tests/fixtures/signing/` workspace.
//! They cover the implemented slice — the `signing:` config surface, profile resolution, and the
//! fail-fast preflight — and assert that signing *execution* (not yet wired) is refused rather than
//! silently skipped. No Docker or network is involved: every signing path resolves before the engine.
//!
//! The `keys/db.{key,crt}` fixtures are PEM-armored stubs (header only), not real key material —
//! enough for the preflight's PEM-shape check.

use std::path::PathBuf;

use assert_cmd::Command;
use predicates::prelude::*;

fn signing_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/signing")
}

fn tailor() -> Command {
    let mut cmd = Command::cargo_bin("tailor").unwrap();
    cmd.current_dir(signing_dir());
    cmd
}

#[test]
fn validate_reports_a_ready_signing_profile_non_fatally() {
    tailor()
        .args(["validate", "ok-default"])
        .assert()
        .success()
        .stdout(
            predicate::str::contains("ok-default")
                .and(predicate::str::contains("signing profile `test-ca` ready")),
        );
}

#[test]
fn validate_reports_a_missing_prerequisite_without_failing() {
    // `missing-key` uses the `broken` keypair profile whose key path does not exist.
    tailor()
        .args(["validate", "missing-key"])
        .assert()
        .success()
        .stdout(
            predicate::str::contains("not ready")
                .and(predicate::str::contains("broken"))
                .and(predicate::str::contains("cannot read `key`")),
        );
}

#[test]
fn build_fails_fast_when_a_signing_key_is_missing() {
    // Fail-fast preflight (§5.1): aborts before any IC run, naming the unmet prerequisite.
    tailor()
        .args(["build", "missing-key"])
        .assert()
        .failure()
        .stderr(
            predicate::str::contains("signing preflight failed")
                .and(predicate::str::contains("cannot read `key`"))
                .and(predicate::str::contains("missing-key")),
        );
}

#[test]
fn build_refuses_a_signed_image_until_execution_is_implemented() {
    // Prerequisites satisfied (local-test-ca is always ready), but execution is unimplemented:
    // tailor refuses rather than emit a silently-unsigned image.
    tailor()
        .args(["build", "ok-default"])
        .assert()
        .failure()
        .stderr(
            predicate::str::contains("not yet implemented").and(predicate::str::contains(
                "Refusing to build a silently-unsigned image",
            )),
        );
}

#[test]
fn build_byo_passes_preflight_then_refuses_execution() {
    // `ok-byo` references the committed PEM stubs, so the keypair preflight passes; the build then
    // stops at the not-yet-implemented gate (proving the preflight succeeded).
    tailor()
        .args(["build", "ok-byo"])
        .assert()
        .failure()
        .stderr(
            predicate::str::contains("not yet implemented").and(predicate::str::contains("byo")),
        );
}

#[test]
fn dry_run_of_a_signed_image_is_daemon_free_and_notes_signing() {
    tailor()
        .args(["build", "--dry-run", "ok-default"])
        .assert()
        .success()
        .stdout(
            predicate::str::contains("(dry-run)")
                .and(predicate::str::contains("signing profile `test-ca` ready"))
                .and(predicate::str::contains(
                    "signing execution is not yet implemented",
                )),
        );
}

#[test]
fn an_unknown_signing_profile_is_a_clear_error() {
    tailor()
        .args(["validate", "bad-profile"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("unknown signing profile `nope`"));
}

#[test]
fn an_unsigned_image_is_unaffected_by_signing() {
    // No `signing:` ⇒ no signing report, and the dry-run renders as usual.
    tailor()
        .args(["build", "--dry-run", "unsigned"])
        .assert()
        .success()
        .stdout(
            predicate::str::contains("(dry-run)")
                .and(predicate::str::contains("signing profile").not()),
        );
}
