//! Backend-agnostic DDC/CI monitor control.
//!
//! `ddc-core` contains everything needed to discover monitors and read/write
//! MCCS VCP features (brightness, contrast, ...) without depending on a GUI
//! toolkit. Concrete transports implement the [`DdcBackend`] trait:
//!
//! * [`ddcutil::DdcutilCli`] shells out to the `ddcutil` command line tool.
//! * [`mock::MockBackend`] is an in-memory backend used by tests and the demo UI.
//!
//! The upstream C++ `ddcui` puts the equivalent logic in its `src/nongui`
//! directory; this crate is the Rust replacement for that layer.

pub mod backend;
pub mod ddcutil;
pub mod error;
pub mod mock;
pub mod model;

pub use backend::DdcBackend;
pub use ddcutil::DdcutilCli;
pub use error::{DdcError, Result};
pub use model::{unique_keys, MonitorId, VcpCode, VcpValue};
