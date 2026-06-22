//! In-memory fakes for testing port consumers without Docker or a network (`meta/docs/architecture.md`
//! §8.3). The orchestrator and downstream crates use these to exercise the build pipeline.

use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
};

use tailor_config::{Arch, BaseSource, ToolchainEntry};
use tokio_util::sync::CancellationToken;

use crate::{
    domain::Cell,
    error::{ExecError, ResolveError},
    ports::{
        BaseResolver, ExecutionContext, ExecutionResult, Executor, ResolvedBase, RuntimeConfig,
    },
};

/// A deterministic `BaseResolver`: local paths hash to zeros, registry bases get a fixed digest.
#[derive(Debug, Default, Clone)]
pub struct FakeResolver;

impl BaseResolver for FakeResolver {
    async fn resolve(&self, source: &BaseSource, arch: Arch) -> Result<ResolvedBase, ResolveError> {
        let platform = format!("linux/{arch}");
        Ok(match source {
            BaseSource::Path { .. } => ResolvedBase::LocalFile {
                sha256: [0; 32],
                size: 0,
            },
            BaseSource::Oci { oci } => ResolvedBase::Oci {
                reference: oci.uri.clone(),
                platform,
                digest: "sha256:fakeoci".to_owned(),
            },
            BaseSource::AzureLinux { azure_linux } => ResolvedBase::Oci {
                reference: format!(
                    "mcr.microsoft.com/azurelinux/{}/image/{}",
                    azure_linux.version, azure_linux.variant
                ),
                platform,
                digest: "sha256:fakeazl".to_owned(),
            },
        })
    }

    async fn resolve_toolchain(&self, _toolchain: &ToolchainEntry) -> Result<String, ResolveError> {
        Ok("sha256:faketoolchain".to_owned())
    }
}

/// An `Executor` that records the slug of every cell it is asked to run and produces no files.
#[derive(Debug, Default, Clone)]
pub struct FakeExecutor {
    calls: Arc<Mutex<Vec<String>>>,
}

impl FakeExecutor {
    /// A handle to the recorded invocations (cell slugs, in call order).
    pub fn recorder(&self) -> Arc<Mutex<Vec<String>>> {
        Arc::clone(&self.calls)
    }
}

impl Executor for FakeExecutor {
    async fn execute(
        &self,
        cell: &Cell,
        context: &ExecutionContext,
        _cancel: CancellationToken,
    ) -> Result<ExecutionResult, ExecError> {
        let slug = cell.slug.0.clone();
        let artifact_path = context.output_dir.join(&slug);
        if let Ok(mut calls) = self.calls.lock() {
            calls.push(slug);
        }
        Ok(ExecutionResult {
            artifact_path,
            exit_code: 0,
            logs: String::new(),
        })
    }

    async fn clean(
        &self,
        _paths: &[PathBuf],
        _runtime: &RuntimeConfig,
        _cancel: CancellationToken,
    ) -> Result<(), ExecError> {
        Ok(())
    }
}
