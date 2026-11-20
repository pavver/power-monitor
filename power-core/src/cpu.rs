//! Processor load from two snapshots of FreeRTOS run-time counters.
//!
//! Each task's counter grows by the time it ran on its core; the total grows
//! with wall time. Both are 32-bit and wrap, so only differences count. A
//! core's load is the share of the interval its idle task did not run.

/// One task in a snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TaskTime {
    /// FreeRTOS's task number: unique for the task's life.
    pub id: u32,
    /// The idle task of this core, when it is one.
    pub idle_of: Option<usize>,
    /// Time the task has run, in counter ticks.
    pub run: u32,
}

/// Every task's run time at one moment.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Snapshot {
    /// Wall time in counter ticks.
    pub total: u32,
    pub tasks: Vec<TaskTime>,
}

/// Load over the interval between two snapshots, in percent of one core.
#[derive(Debug, Clone, PartialEq)]
pub struct Load {
    /// Per core, by core number.
    pub cores: Vec<f32>,
    /// Per task id present in both snapshots.
    pub tasks: Vec<(u32, f32)>,
}

/// The load between `before` and `after`, or `None` when no time passed.
pub fn load(before: &Snapshot, after: &Snapshot, cores: usize) -> Option<Load> {
    let wall = after.total.wrapping_sub(before.total);
    if wall == 0 {
        return None;
    }
    let share = |ran: u32| (ran as f32 * 100.0 / wall as f32).min(100.0);
    let tasks: Vec<(u32, f32)> = after
        .tasks
        .iter()
        .filter_map(|now| {
            let was = before.tasks.iter().find(|t| t.id == now.id)?;
            Some((now.id, share(now.run.wrapping_sub(was.run))))
        })
        .collect();
    let idle = |core: usize| {
        let task = after.tasks.iter().find(|t| t.idle_of == Some(core))?;
        tasks.iter().find(|(id, _)| *id == task.id).map(|(_, p)| *p)
    };
    let cores = (0..cores).map(|core| 100.0 - idle(core).unwrap_or(100.0)).collect();
    Some(Load { cores, tasks })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task(id: u32, idle_of: Option<usize>, run: u32) -> TaskTime {
        TaskTime { id, idle_of, run }
    }

    #[test]
    fn shares_of_the_interval() {
        let before = Snapshot { total: 1_000, tasks: vec![task(1, Some(0), 500), task(2, Some(1), 0), task(3, None, 100)] };
        let after = Snapshot { total: 2_000, tasks: vec![task(1, Some(0), 1_250), task(2, Some(1), 1_000), task(3, None, 350)] };
        let load = load(&before, &after, 2).unwrap();
        assert_eq!(load.cores, vec![25.0, 0.0]);
        assert_eq!(load.tasks, vec![(1, 75.0), (2, 100.0), (3, 25.0)]);
    }

    #[test]
    fn counters_that_wrapped() {
        let before = Snapshot { total: u32::MAX - 99, tasks: vec![task(1, Some(0), u32::MAX - 9)] };
        let after = Snapshot { total: 100, tasks: vec![task(1, Some(0), 40)] };
        let load = load(&before, &after, 1).unwrap();
        assert_eq!(load.tasks, vec![(1, 25.0)]);
        assert_eq!(load.cores, vec![75.0]);
    }

    #[test]
    fn a_task_born_in_between_has_no_share_yet() {
        let before = Snapshot { total: 0, tasks: vec![task(1, Some(0), 0)] };
        let after = Snapshot { total: 100, tasks: vec![task(1, Some(0), 100), task(9, None, 30)] };
        assert_eq!(load(&before, &after, 1).unwrap().tasks, vec![(1, 100.0)]);
    }

    #[test]
    fn no_time_no_load() {
        let same = Snapshot { total: 5, tasks: vec![] };
        assert_eq!(load(&same, &same, 2), None);
    }

    #[test]
    fn a_core_without_its_idle_task_counts_as_idle() {
        let before = Snapshot { total: 0, tasks: vec![] };
        let after = Snapshot { total: 10, tasks: vec![] };
        assert_eq!(load(&before, &after, 2).unwrap().cores, vec![0.0, 0.0]);
    }
}
