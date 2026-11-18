//! Threads under their own names, and how much of each task's stack was
//! never touched: the numbers to size the stacks by.

use std::ffi::CStr;
use std::io;
use std::thread::{self, JoinHandle};

use esp_idf_svc::hal::task::thread::ThreadSpawnConfiguration;
use esp_idf_svc::sys::{uxTaskGetNumberOfTasks, uxTaskGetSystemState, TaskStatus_t};

/// Spawns `run` on a thread whose FreeRTOS task is called `name`
/// (at most 15 bytes), so it can be told apart in [`stack_reserve`].
pub fn spawn<T: Send + 'static>(
    name: &'static CStr,
    stack_size: usize,
    run: impl FnOnce() -> T + Send + 'static,
) -> io::Result<JoinHandle<T>> {
    // The configuration is the calling thread's own, so threads spawned
    // elsewhere at the same time are not affected.
    configure(ThreadSpawnConfiguration { name: Some(name), stack_size, ..Default::default() })?;
    let spawned = thread::Builder::new().stack_size(stack_size).spawn(run);
    configure(ThreadSpawnConfiguration::default())?;
    spawned
}

fn configure(configuration: ThreadSpawnConfiguration) -> io::Result<()> {
    configuration.set().map_err(io::Error::other)
}

/// Every task's name and the fewest bytes of its stack that stayed free
/// since it started.
pub fn stack_reserve() -> serde_json::Map<String, serde_json::Value> {
    // SAFETY: the array has room for every task counted, and FreeRTOS
    // fills no more entries than it is told there is room for.
    let tasks = unsafe {
        let mut tasks = vec![std::mem::zeroed::<TaskStatus_t>(); uxTaskGetNumberOfTasks() as usize + 2];
        let filled = uxTaskGetSystemState(tasks.as_mut_ptr(), tasks.len() as _, std::ptr::null_mut());
        tasks.truncate(filled as usize);
        tasks
    };
    tasks
        .iter()
        .map(|task| {
            // SAFETY: FreeRTOS task names are NUL-terminated and live as long as the task.
            let name = unsafe { CStr::from_ptr(task.pcTaskName) }.to_string_lossy().into_owned();
            (name, task.usStackHighWaterMark.into())
        })
        .collect()
}
