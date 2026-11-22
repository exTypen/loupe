//! The virtualized diff renderer: `DiffView` (view.rs) over the types in model.rs.
pub mod model;
mod view;

pub use model::*;
pub use view::{DiffView, Highlighter};
