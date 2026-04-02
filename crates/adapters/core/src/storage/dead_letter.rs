//! Dead letter queue storage — backed by SurrealDB via synapse_memory.
//!
//! Phase 4.5: The DLQ table lives in the same SurrealDB instance as the
//! memory system (schema defined in `surrealdb_schema.surql`).
//! `DeadLetterPort` is implemented on `SurrealMemoryAdapter` in the memory crate.
//!
//! This module is kept as a placeholder for the `storage::dead_letter` path.
//! The actual adapter is in `synapse_memory::surrealdb_adapter`.
