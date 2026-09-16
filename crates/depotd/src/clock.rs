use std::time::{SystemTime, UNIX_EPOCH};

use depot_core::Timestamp;

pub fn now() -> Timestamp {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or_default();
    Timestamp::from_millis(millis)
}
