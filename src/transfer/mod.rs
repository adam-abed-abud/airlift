//! Moving bytes: streamed downloads (with HTTP range support) and, when
//! enabled, uploads. Files are never read fully into memory.

pub mod download;
pub mod range;
