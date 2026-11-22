//! Stable identifier helpers. Ids are UUIDv4 strings; they never change when
//! a segment is re-run, which keeps evidence and audit references valid.

use uuid::Uuid;

pub fn new_id(prefix: &str) -> String {
    format!("{prefix}_{}", Uuid::new_v4().simple())
}
