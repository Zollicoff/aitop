//! Turn the event list into what the UI and reports display.

mod block;
mod range;
mod rows;
mod summary;
#[cfg(test)]
pub(crate) mod testutil;

pub use block::Block;
pub use range::{local_midnight, Bucket, Range, Sort};
pub use rows::Row;
pub use summary::{daily, summarize, Summary, ToolSummary};
