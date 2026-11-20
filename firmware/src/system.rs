//! The resource monitor behind `/api/system`: processor load, every task's
//! stack, memory, NVS and flash, read when asked.

use std::ptr;
use std::sync::{Mutex, OnceLock};

use esp_idf_svc::sys::{self, TaskStatus_t};
use power_core::cpu::{self, Snapshot, TaskTime};
use serde_json::{json, Value};

const CORES: usize = sys::CONFIG_FREERTOS_NUMBER_OF_CORES as usize;
/// NVS keeps data in 32-byte entries.
const NVS_ENTRY: usize = 32;

/// The run-time counters of the previous request: load is the change since.
static LAST: Mutex<Option<Snapshot>> = Mutex::new(None);

pub fn view() -> Value {
    let (tasks, total) = tasks();
    let now = Snapshot {
        total,
        tasks: tasks.iter().map(|t| TaskTime { id: t.id, idle_of: t.idle_of, run: t.run }).collect(),
    };
    let load = {
        let mut last = LAST.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let load = last.as_ref().and_then(|before| cpu::load(before, &now, CORES));
        *last = Some(now);
        load
    };
    let share = |id: u32| load.as_ref()?.tasks.iter().find(|(task, _)| *task == id).map(|(_, p)| *p);
    let tasks: Vec<Value> = tasks
        .iter()
        .map(|t| {
            json!({
                "name": t.name, "core": t.core, "priority": t.priority,
                "stack": t.stack, "stack_free_min": t.stack_free_min, "cpu": share(t.id),
            })
        })
        .collect();
    json!({
        "uptime_s": crate::status::now_ms() / 1000,
        "reset_reason": format!("{:?}", esp_idf_svc::hal::reset::ResetReason::get()),
        "rssi": rssi(),
        "cpu": load.map(|l| l.cores),
        "tasks": tasks,
        "ram": ram(),
        "nvs": nvs(),
        "flash": flash(),
    })
}

struct Task {
    name: String,
    id: u32,
    idle_of: Option<usize>,
    /// The core it is pinned to; `None` when it may run on either.
    core: Option<usize>,
    priority: u32,
    stack: Option<usize>,
    stack_free_min: u32,
    run: u32,
}

/// Every task and the run-time counter's total.
fn tasks() -> (Vec<Task>, u32) {
    // SAFETY: a plain count; a few spare slots cover tasks started meanwhile.
    let room = unsafe { sys::uxTaskGetNumberOfTasks() } as usize + 4;
    let mut raw: Vec<TaskStatus_t> = Vec::with_capacity(room);
    let mut total = 0;
    // SAFETY: FreeRTOS fills at most `room` entries and returns how many.
    let filled = unsafe { sys::uxTaskGetSystemState(raw.as_mut_ptr(), room as u32, &mut total) } as usize;
    // SAFETY: the first `filled` entries were written above.
    unsafe { raw.set_len(filled.min(room)) };
    let tasks = raw
        .iter()
        .map(|s| {
            // SAFETY: the name lives as long as the task, which runs now; it is copied at once.
            let freertos_name = unsafe { std::ffi::CStr::from_ptr(s.pcTaskName) }.to_string_lossy().into_owned();
            let (name, stack) = crate::tasks::describe(s.xHandle, &freertos_name);
            // SAFETY: the handle came from FreeRTOS for a task that runs now.
            let core = usize::try_from(unsafe { sys::xTaskGetCoreID(s.xHandle) }).ok().filter(|&c| c < CORES);
            let idle_of = freertos_name.strip_prefix("IDLE").and_then(|n| n.parse().ok());
            Task {
                name,
                id: s.xTaskNumber,
                idle_of,
                core,
                priority: s.uxCurrentPriority,
                stack,
                stack_free_min: s.usStackHighWaterMark,
                run: s.ulRunTimeCounter,
            }
        })
        .collect();
    (tasks, total)
}

fn ram() -> Value {
    let caps = sys::MALLOC_CAP_8BIT;
    // SAFETY: plain reads of heap statistics.
    unsafe {
        json!({
            "total": sys::heap_caps_get_total_size(caps),
            "free": sys::heap_caps_get_free_size(caps),
            "free_min": sys::heap_caps_get_minimum_free_size(caps),
            "largest_block": sys::heap_caps_get_largest_free_block(caps),
        })
    }
}

fn nvs() -> Value {
    let mut stats = sys::nvs_stats_t::default();
    // SAFETY: null names the default "nvs" partition; `stats` is written on success.
    if unsafe { sys::nvs_get_stats(ptr::null(), &mut stats) } != sys::ESP_OK {
        return Value::Null;
    }
    json!({
        "used": stats.used_entries * NVS_ENTRY,
        "free": stats.available_entries * NVS_ENTRY,
        "total": stats.total_entries * NVS_ENTRY,
        "namespaces": stats.namespace_count,
    })
}

fn flash() -> Value {
    let mut chip = 0;
    // SAFETY: null stands for the default flash chip.
    unsafe { sys::esp_flash_get_size(ptr::null_mut(), &mut chip) };
    // SAFETY: the running partition is a static table entry, or null.
    let slot = unsafe { sys::esp_ota_get_running_partition().as_ref() };
    json!({
        "chip": chip,
        "slot": slot.map(|p| p.size),
        "app": app_size(),
    })
}

/// The running image's length: reading it hashes the whole image, so once.
fn app_size() -> Option<u32> {
    static SIZE: OnceLock<Option<u32>> = OnceLock::new();
    *SIZE.get_or_init(|| {
        // SAFETY: the running partition is a static table entry, or null.
        let slot = unsafe { sys::esp_ota_get_running_partition().as_ref() }?;
        let pos = sys::esp_partition_pos_t { offset: slot.address, size: slot.size };
        let mut meta = sys::esp_image_metadata_t::default();
        // SAFETY: both pointers are valid for the call.
        (unsafe { sys::esp_image_get_metadata(&pos, &mut meta) } == sys::ESP_OK).then_some(meta.image_len)
    })
}

/// Signal strength of the access point the device is joined to, in dBm.
fn rssi() -> Option<i8> {
    let mut ap = sys::wifi_ap_record_t::default();
    // SAFETY: `ap` is written when the station is connected.
    (unsafe { sys::esp_wifi_sta_get_ap_info(&mut ap) } == sys::ESP_OK).then_some(ap.rssi)
}
