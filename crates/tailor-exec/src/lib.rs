//! Container execution adapter for Image Customizer.

mod arg_builder;
mod color;
mod container;
mod executor;
mod ic_log;
mod janitor;
mod output_artifacts;
pub mod path_translate;
pub mod rpm_farm;
pub mod working_copy;

pub use arg_builder::build_ic_args;
pub use color::color_enabled;
pub use container::connection::{ConnectionPlan, Endpoint, Resolution, ResolveInputs, resolve};
pub use container::runtime::{BollardRuntime, NoopRuntime};
pub use executor::IcExecutor;
