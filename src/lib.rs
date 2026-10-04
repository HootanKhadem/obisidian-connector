//! Obsidian connector: gives any AI agent safe access to an Obsidian vault
//! via the Model Context Protocol (MCP) or a plain JSON command line.

pub mod discovery;
pub mod edit;
pub mod markdown;
pub mod mcp;
pub mod setup;
pub mod tools;
pub mod vault;
