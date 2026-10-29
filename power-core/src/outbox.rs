//! Messages waiting for delivery. Wi-Fi usually drops together with the power,
//! so they pile up here until the network is back.

use std::collections::VecDeque;

pub struct Outbox<T> {
    pending: VecDeque<T>,
    capacity: usize,
}

impl<T> Outbox<T> {
    pub fn new(capacity: usize) -> Self {
        Self { pending: VecDeque::with_capacity(capacity), capacity }
    }

    /// Queues `item`; when full, the oldest one is dropped to make room.
    pub fn push(&mut self, item: T) {
        if self.pending.len() == self.capacity {
            self.pending.pop_front();
        }
        self.pending.push_back(item);
    }

    /// The next item to send; it stays queued until `delivered`.
    pub fn next(&self) -> Option<&T> {
        self.pending.front()
    }

    pub fn delivered(&mut self) {
        self.pending.pop_front();
    }

    pub fn len(&self) -> usize {
        self.pending.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::power::{PowerEvent, PowerState};

    fn event(at_ms: u64) -> PowerEvent {
        PowerEvent { state: PowerState::Absent, at_ms, previous_lasted_ms: None }
    }

    #[test]
    fn events_go_out_in_order_and_stay_until_delivered() {
        let mut outbox = Outbox::new(4);
        assert_eq!(outbox.next(), None);
        outbox.push(event(1));
        outbox.push(event(2));
        assert_eq!(outbox.next(), Some(&event(1)));
        assert_eq!(outbox.next(), Some(&event(1)));
        outbox.delivered();
        assert_eq!(outbox.next(), Some(&event(2)));
        assert_eq!(outbox.len(), 1);
    }

    #[test]
    fn full_outbox_drops_the_oldest_event() {
        let mut outbox = Outbox::new(2);
        for at_ms in 1..=3 {
            outbox.push(event(at_ms));
        }
        assert_eq!(outbox.len(), 2);
        assert_eq!(outbox.next(), Some(&event(2)));
    }

    #[test]
    fn delivered_on_an_empty_outbox_does_nothing() {
        let mut outbox: Outbox<PowerEvent> = Outbox::new(2);
        outbox.delivered();
        assert!(outbox.is_empty());
    }
}
