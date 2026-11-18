//! Our threads, and how much of each task's stack was never touched: the
//! numbers to size the stacks by.
//!
//! Threads are spawned plainly. Naming them through esp-idf-hal's
//! `ThreadSpawnConfiguration` gave a new thread a stack over memory still in
//! use: the main task's locals turned to 0xa5 fill and the device crashed on
//! its first Wi-Fi join. So each thread records its own task here instead.

use std::ffi::CStr;
use std::io;
use std::sync::Mutex;
use std::thread::{self, JoinHandle};

use esp_idf_svc::sys::{uxTaskGetStackHighWaterMark, xTaskGetCurrentTaskHandle, xTaskGetHandle, TaskHandle_t};

/// ESP-IDF's own tasks that live as long as the device; a name not running
/// (Bluetooth without a BMS) is left out.
const SYSTEM_TASKS: [&CStr; 12] = [
    c"IDLE0", c"IDLE1", c"Tmr Svc", c"esp_timer", c"ipc0", c"ipc1", c"sys_evt", c"tiT", c"wifi", c"httpd",
    c"nimble_host", c"btController",
];

/// Our running threads, by task. A thread leaves before its task ends, and
/// the lock is held while a task is measured, so no entry outlives its task.
static OURS: Mutex<Vec<(&'static str, Task)>> = Mutex::new(Vec::new());

#[derive(Clone, Copy, PartialEq)]
struct Task(TaskHandle_t);

// SAFETY: a task handle is an address FreeRTOS hands out to any task.
unsafe impl Send for Task {}

/// Spawns `run` on a thread whose stack shows up as `name` in [`stack_reserve`].
pub fn spawn<T: Send + 'static>(
    name: &'static str,
    stack_size: usize,
    run: impl FnOnce() -> T + Send + 'static,
) -> io::Result<JoinHandle<T>> {
    thread::Builder::new().stack_size(stack_size).spawn(move || {
        let _listed = Listed::new(name);
        run()
    })
}

/// The current task on the list while this lives.
struct Listed(Task);

impl Listed {
    fn new(name: &'static str) -> Self {
        // SAFETY: plain query of the running task.
        let task = Task(unsafe { xTaskGetCurrentTaskHandle() });
        lock().push((name, task));
        Self(task)
    }
}

impl Drop for Listed {
    fn drop(&mut self) {
        lock().retain(|(_, task)| *task != self.0);
    }
}

fn lock() -> std::sync::MutexGuard<'static, Vec<(&'static str, Task)>> {
    OURS.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Every task's name and the fewest bytes of its stack that stayed free
/// since it started.
pub fn stack_reserve() -> serde_json::Map<String, serde_json::Value> {
    let ours = lock();
    let system = SYSTEM_TASKS.iter().filter_map(|name| {
        // SAFETY: a NUL-terminated name; a null handle means no such task.
        let task = unsafe { xTaskGetHandle(name.as_ptr()) };
        (!task.is_null()).then(|| (name.to_string_lossy().into_owned(), task))
    });
    let mine = ours.iter().map(|(name, task)| (name.to_string(), task.0));
    system
        .chain(mine)
        // SAFETY: each task lives: ESP-IDF's never end, ours are listed only while running.
        .map(|(name, task)| (name, unsafe { uxTaskGetStackHighWaterMark(task) }.into()))
        .collect()
}
