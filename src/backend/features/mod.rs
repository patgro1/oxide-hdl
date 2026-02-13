//! LSP feature implementations for Oxide HDL.
//!
//! This module contains handlers for various LSP features:
//! * [`completion`]: Code completion suggestions
//! * [`diagnostics`]: Error and warning detection
//! * [`goto`]: Go to definition functionality
//! * [`hover`]: Hover information display
//! * [`lookup`]: Symbol resolution and lookup
//! * [`rename`]: Symbol rename operations
//! * [`symbol`]: Document and workspace symbol providers

pub mod completion;
pub mod diagnostics;
pub mod goto;
pub mod hover;
pub mod lookup;
pub mod rename;
pub mod symbol;
