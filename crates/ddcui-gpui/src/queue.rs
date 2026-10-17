//! The write queue that keeps level-bar dragging from flooding a monitor.
//!
//! Writes are serialized (one command at a time) and coalesced per monitor and
//! feature: when several values are asked for before the worker gets to a row,
//! only the newest one is written. That is all the hardware needs, and it is
//! what keeps a drag from queueing up hundreds of `ddcutil` calls.
//!
//! The queue is deliberately free of GUI types so it can be tested on its own.

/// A write that has been asked for but not handed to the worker yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WriteJob {
    /// The monitor's stable key, so a write cannot land on a different monitor
    /// when the list is refreshed while the queue is busy.
    pub key: String,
    /// Index of the feature in that monitor's feature list.
    pub feature: usize,
    /// Value to write, as a percentage of the feature's range.
    pub percent: u8,
    /// Value to show again if the write fails.
    pub previous: u16,
}

/// Writes waiting for the worker, newest value per row.
#[derive(Debug, Default)]
pub struct WriteQueue {
    jobs: Vec<WriteJob>,
}

impl WriteQueue {
    /// Queue a write, replacing any earlier one for the same row.
    ///
    /// Returns whether an earlier write was replaced. Either way the newest
    /// value wins, and rows keep their relative order so one busy monitor
    /// cannot starve another.
    pub fn push(&mut self, job: WriteJob) -> bool {
        for slot in self.jobs.iter_mut() {
            if slot.key == job.key && slot.feature == job.feature {
                *slot = job;
                return true;
            }
        }
        self.jobs.push(job);
        false
    }

    /// Take the oldest queued write.
    pub fn pop(&mut self) -> Option<WriteJob> {
        if self.jobs.is_empty() {
            return None;
        }
        Some(self.jobs.remove(0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job(monitor: &str, feature: usize, percent: u8) -> WriteJob {
        WriteJob {
            key: monitor.to_string(),
            feature,
            percent,
            previous: 0,
        }
    }

    #[test]
    fn a_queued_row_keeps_only_its_newest_value() {
        let mut queue = WriteQueue::default();

        assert!(!queue.push(job("left", 0, 10)));
        assert!(queue.push(job("left", 0, 20)));
        assert!(queue.push(job("left", 0, 30)));

        assert_eq!(queue.pop(), Some(job("left", 0, 30)));
        assert_eq!(queue.pop(), None);
    }

    #[test]
    fn different_rows_are_kept_separately_in_order() {
        let mut queue = WriteQueue::default();
        queue.push(job("left", 0, 10));
        queue.push(job("right", 0, 20));
        queue.push(job("right", 1, 30));

        // Refreshing an old row does not move it to the front or drop it.
        queue.push(job("left", 0, 15));

        assert_eq!(queue.pop(), Some(job("left", 0, 15)));
        assert_eq!(queue.pop(), Some(job("right", 0, 20)));
        assert_eq!(queue.pop(), Some(job("right", 1, 30)));
        assert_eq!(queue.pop(), None);
    }

    #[test]
    fn an_empty_queue_stays_empty() {
        let mut queue = WriteQueue::default();
        assert_eq!(queue.pop(), None);
        assert_eq!(queue.pop(), None);
    }
}
