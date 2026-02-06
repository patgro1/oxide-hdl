//! Syntax parsing and scanning utilities for VHDL files.
//!
//! This module contains:
//! * [`parser`]: Full Tree-sitter based VHDL parsing
//! * [`scanner`]: Fast regex-based scanning for workspace indexing
//! * [`utils`]: Helper functions for working with the parsed syntax tree

pub mod parser;
pub mod scanner;
pub mod utils;
