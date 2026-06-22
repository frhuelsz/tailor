//! Container execution adapter for Image Customizer.

mod arg_builder;
mod container;
mod executor;
mod janitor;
pub mod path_translate;
pub mod rpm_farm;
pub mod working_copy;

pub use arg_builder::build_ic_args;
pub use container::runtime::BollardRuntime;
pub use executor::IcExecutor;
