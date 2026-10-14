//! Spot-price domain rules without I/O: delivery areas, contract price math,
//! local-day ranges, and the FI forecast. Callers pass time and data in.

pub mod areas;
pub mod endpoints;
pub mod forecast;
pub mod js;
pub mod price;
pub mod query;
pub mod settings;
pub mod time;
