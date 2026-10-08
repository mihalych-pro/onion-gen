//! The master: hands out work, takes in what is found, shows the fleet.

pub mod finds;
pub mod metrics;
pub mod queue;
pub mod serve;
pub mod state;

pub use finds::{examine, Verdict};
pub use queue::{Queue, Range};
pub use serve::{serve, Shared};
pub use state::Master;
