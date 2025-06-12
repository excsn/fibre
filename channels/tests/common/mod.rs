use std::time::Duration;

pub const SHORT_TIMEOUT: Duration = Duration::from_millis(500);
pub const LONG_TIMEOUT: Duration = Duration::from_secs(3);
pub const STRESS_TIMEOUT: Duration = Duration::from_secs(15);
pub const ITEMS_LOW: usize = 50;
pub const ITEMS_MEDIUM: usize = 200;
pub const ITEMS_HIGH: usize = 1000;