//! `tailor-resolve` — base-image and toolchain digest resolution adapters.

mod azure_linux;
mod local;
mod oci;
mod resolver;
mod toolchain;

pub use resolver::OciResolver;
