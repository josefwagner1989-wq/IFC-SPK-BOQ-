//! ifc-core: leitor IFC em streaming e levantamento de quantitativos para sistemas de sprinklers.
pub mod excel;
pub mod geom;
pub mod model;
pub mod step;
pub mod tess;

pub use model::{Filter, Model, ScanSummary, Takeoff};
