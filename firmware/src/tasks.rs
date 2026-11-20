//! Our threads and the stack each task was given: with the fewest bytes
//! never touched (see `system`), the numbers to size the stacks by.
//!
//! Threads are spawned plainly. Naming them through esp-idf-hal's
//! `ThreadSpawnConfiguration` gave a new thread a stack over memory still in
//! use: the main task's locals turned to 0xa5 fill and the device crashed on
//! its first Wi-Fi join. So each thread records its own task here instead.

use std::io;
use std::sync::Mutex;
use std::thread::{self, JoinHandle};

use esp_idf_svc::sys::{self, xTaskGetCurrentTaskHandle, TaskHandle_t};

/// Our running threads: name, task, stack bytes. A thread leaves before its
/// task ends, so no entry outlives its task.
static OURS: Mutex<Vec<(&'static str, Task, usize)>> = Mutex::new(Vec::new());

#[derive(Clone, Copy, PartialEq)]
struct Task(TaskHandle_t);

// SAFETY: a task handle is an address FreeRTOS hands out to any task.
unsafe impl Send for Task {}

/// Spawns `run` on a thread listed as `name` with `stack_size` bytes of stack.
pub fn spawn<T: Send + 'static>(
    name: &'static str,
    stack_size: usize,
    run: impl FnOnce() -> T + Send + 'static,
) -> io::Result<JoinHandle<T>> {
    thread::Builder::new().stack_size(stack_size).spawn(move || {
        let _listed = Listed::new(name, stack_size);
        run()
    })
}

/// The current task on the list while this lives.
struct Listed(Task);

impl Listed {
    fn new(name: &'static str, stack_size: usize) -> Self {
        // SAFETY: plain query of the running task.
        let task = Task(unsafe { xTaskGetCurrentTaskHandle() });
        lock().push((name, task, stack_size));
        Self(task)
    }
}

impl Drop for Listed {
    fn drop(&mut self) {
        lock().retain(|(_, task, _)| *task != self.0);
    }
}

fn lock() -> std::sync::MutexGuard<'static, Vec<(&'static str, Task, usize)>> {
    OURS.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The name and stack bytes of `task`: ours as spawned, ESP-IDF's from its
/// configuration; `None` for a task whose stack the configuration does not
/// set (Wi-Fi, Bluetooth controller). A thread of ours runs as "pthread".
pub fn describe(task: TaskHandle_t, freertos_name: &str) -> (String, Option<usize>) {
    if let Some((name, _, size)) = lock().iter().find(|(_, t, _)| t.0 == task) {
        return (name.to_string(), Some(*size));
    }
    let size = match freertos_name {
        "IDLE0" | "IDLE1" => Some(sys::CONFIG_FREERTOS_IDLE_TASK_STACKSIZE),
        "ipc0" | "ipc1" => Some(sys::CONFIG_ESP_IPC_TASK_STACK_SIZE),
        "esp_timer" => Some(sys::CONFIG_ESP_TIMER_TASK_STACK_SIZE),
        "sys_evt" => Some(sys::CONFIG_ESP_SYSTEM_EVENT_TASK_STACK_SIZE),
        "tiT" => Some(sys::CONFIG_LWIP_TCPIP_TASK_STACK_SIZE),
        "Tmr Svc" => Some(sys::CONFIG_FREERTOS_TIMER_TASK_STACK_DEPTH),
        "main" => Some(sys::CONFIG_ESP_MAIN_TASK_STACK_SIZE),
        "nimble_host" => Some(sys::CONFIG_BT_NIMBLE_HOST_TASK_STACK_SIZE),
        "httpd" => Some(crate::web::STACK as u32),
        _ => None,
    };
    (freertos_name.to_string(), size.map(|s| s as usize))
}
