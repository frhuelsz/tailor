//! Build script: embed the full version string as the `TAILOR_VERSION` env var, consumed by the
//! CLI for both `--version` and the `version` subcommand.
//!
//! The string is the Cargo (SemVer) version followed by **build metadata** per SemVer §10:
//! `<x.y.z>+<short-commit>.<YYYY-MM-DD>`. Build-metadata identifiers are dot-separated and limited
//! to `[0-9A-Za-z-]`, so a hyphenated ISO date is valid. The date honours `SOURCE_DATE_EPOCH` for
//! reproducible builds; the commit falls back to `unknown` outside a git checkout (e.g. no commits).

use std::{
    env,
    path::Path,
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

fn main() {
    let pkg_version = env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| "0.0.0".to_owned());
    let commit = git(&["rev-parse", "--short", "HEAD"]).unwrap_or_else(|| "unknown".to_owned());
    let date = build_date();
    println!("cargo::rustc-env=TAILOR_VERSION={pkg_version}+{commit}.{date}");

    // The date input and the checked-out commit are the only things that change the version.
    println!("cargo::rerun-if-env-changed=SOURCE_DATE_EPOCH");
    if let Some(git_dir) = git(&["rev-parse", "--absolute-git-dir"]) {
        let head = Path::new(&git_dir).join("HEAD");
        if head.exists() {
            println!("cargo::rerun-if-changed={}", head.display());
        }
        if let Some(branch) = git(&["symbolic-ref", "-q", "--short", "HEAD"]) {
            let reference = Path::new(&git_dir).join("refs").join("heads").join(branch);
            if reference.exists() {
                println!("cargo::rerun-if-changed={}", reference.display());
            }
        }
    }
}

/// Run `git <args>`, returning trimmed stdout on success, or `None` if git is absent/failed.
fn git(args: &[&str]) -> Option<String> {
    let output = Command::new("git").args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8(output.stdout).ok()?.trim().to_owned();
    if text.is_empty() { None } else { Some(text) }
}

/// The UTC build date as `YYYY-MM-DD`, honouring `SOURCE_DATE_EPOCH` when set.
fn build_date() -> String {
    let epoch = env::var("SOURCE_DATE_EPOCH")
        .ok()
        .and_then(|value| value.parse::<i64>().ok())
        .or_else(|| {
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .ok()
                .and_then(|elapsed| i64::try_from(elapsed.as_secs()).ok())
        })
        .unwrap_or(0);
    let (year, month, day) = civil_from_days(epoch.div_euclid(86_400));
    format!("{year:04}-{month:02}-{day:02}")
}

/// Convert days since the Unix epoch (1970-01-01) to a `(year, month, day)` UTC civil date.
/// Howard Hinnant's algorithm: <https://howardhinnant.github.io/date_algorithms.html#civil_from_days>.
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = (if z >= 0 { z } else { z - 146_096 }) / 146_097;
    let day_of_era = z - era * 146_097; // [0, 146096]
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365; // [0, 399]
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100); // [0, 365]
    let mp = (5 * day_of_year + 2) / 153; // [0, 11]
    let day = day_of_year - (153 * mp + 2) / 5 + 1; // [1, 31]
    let month = if mp < 10 { mp + 3 } else { mp - 9 }; // [1, 12]
    (year + i64::from(month <= 2), month, day)
}
