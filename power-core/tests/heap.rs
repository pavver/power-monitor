use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

use power_core::json::Count;
use power_core::settings::{Destination, Messages, Text};
use power_core::telegram::{read_chats, write_send_message};
use power_core::template;

struct TrackingAllocator;

static CURRENT: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for TrackingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let ptr = System.alloc(layout);
        if !ptr.is_null() {
            let current = CURRENT.fetch_add(layout.size(), Ordering::SeqCst) + layout.size();
            PEAK.fetch_max(current, Ordering::SeqCst);
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        System.dealloc(ptr, layout);
        CURRENT.fetch_sub(layout.size(), Ordering::SeqCst);
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let new_ptr = System.realloc(ptr, layout, new_size);
        if !new_ptr.is_null() {
            if new_size > layout.size() {
                let diff = new_size - layout.size();
                let current = CURRENT.fetch_add(diff, Ordering::SeqCst) + diff;
                PEAK.fetch_max(current, Ordering::SeqCst);
            } else {
                let diff = layout.size() - new_size;
                CURRENT.fetch_sub(diff, Ordering::SeqCst);
            }
        }
        new_ptr
    }
}

#[global_allocator]
static ALLOCATOR: TrackingAllocator = TrackingAllocator;

fn peak_during<R>(f: impl FnOnce() -> R) -> (R, usize) {
    let start = CURRENT.load(Ordering::SeqCst);
    PEAK.store(start, Ordering::SeqCst);
    let res = f();
    let peak = PEAK.load(Ordering::SeqCst);
    (res, peak.saturating_sub(start))
}

struct ChunkedReader<'a> {
    data: &'a [u8],
    max_chunk: usize,
}

impl<'a> std::io::Read for ChunkedReader<'a> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.data.is_empty() {
            return Ok(0);
        }
        let take = self.data.len().min(buf.len()).min(self.max_chunk);
        buf[..take].copy_from_slice(&self.data[..take]);
        self.data = &self.data[take..];
        Ok(take)
    }
}

fn build_updates_json(count: usize) -> String {
    let cyrillic_text = "Повідомлення про стан електромережі. ".repeat(110); // ~4000 chars
    let mut json = String::from("{\"ok\":true,\"result\":[");
    for i in 0..count {
        if i > 0 {
            json.push(',');
        }
        let (chat_id, chat_type, title, is_forum) = match i % 3 {
            0 => (-1001, "group", "Сім'я", false),
            1 => (-1002, "supergroup", "Будинок", true),
            _ => (42, "private", "Олена", false),
        };
        let forum_field = if is_forum { ",\"is_forum\":true" } else { "" };
        let entities = (0..50)
            .map(|offset| format!("{{\"type\":\"bold\",\"offset\":{},\"length\":5}}", offset * 10))
            .collect::<Vec<_>>()
            .join(",");
        let update = format!(
            "{{\"update_id\":{i},\"message\":{{\"message_id\":{i},\"chat\":{{\"id\":{chat_id},\"type\":\"{chat_type}\",\"title\":\"{title}\"{forum_field}}},\"text\":\"{cyrillic_text}\",\"entities\":[{entities}]}}}}"
        );
        json.push_str(&update);
    }
    json.push_str("]}");
    json
}

#[test]
fn memory_benchmarks() {
    // 1. read_chats over 100 updates vs 10 updates
    let json100 = build_updates_json(100);
    let json10 = build_updates_json(10);

    let (chats100, peak100) = peak_during(|| {
        let mut reader = ChunkedReader { data: json100.as_bytes(), max_chunk: 256 };
        read_chats(200, &mut reader).unwrap()
    });
    let (chats10, peak10) = peak_during(|| {
        let mut reader = ChunkedReader { data: json10.as_bytes(), max_chunk: 256 };
        read_chats(200, &mut reader).unwrap()
    });

    eprintln!("read_chats peak for 100 updates: {peak100} B");
    eprintln!("read_chats peak for 10 updates: {peak10} B");

    assert_eq!(chats100.len(), 3);
    assert_eq!(chats10.len(), 3);
    assert!(peak100 < 8 * 1024, "Peak for 100 updates was {peak100} B (expected < 8 KB)");
    let diff = (peak100 as isize - peak10 as isize).abs();
    assert!(diff <= 1024, "Difference between 100 and 10 updates was {diff} B (expected <= 1 KB)");

    // 2. write_send_message into Count and into pre-allocated String
    let dest = Destination { chat_id: -1001234567890, thread_id: Some(42) };
    let msg_text = "Привіт! Світло зникло. \n \"quoted\" \\ escaped";

    let mut count = Count(0);
    let (_, peak_count) = peak_during(|| {
        write_send_message(dest, msg_text, &mut count).unwrap();
    });
    eprintln!("write_send_message into Count peak: {peak_count} B");
    assert_eq!(peak_count, 0, "write_send_message into Count allocated {peak_count} B");

    let mut preallocated_str = String::with_capacity(1024);
    let (_, peak_str) = peak_during(|| {
        write_send_message(dest, msg_text, &mut preallocated_str).unwrap();
    });
    eprintln!("write_send_message into String peak: {peak_str} B");
    assert_eq!(peak_str, 0, "write_send_message into String allocated {peak_str} B");

    // 3. template::render_to of longest default template (StillOff)
    let messages = Messages::default();
    let template = messages.template(Text::StillOff);
    let values = [
        ("device", Some("Дім")),
        ("duration", Some("2 год 15 хв")),
        ("delay", Some("5 хв")),
        ("bms_soc", Some("84")),
        ("bms_voltage", Some("52.4")),
        ("bms_energy", Some("1.23")),
        ("battery_voltage", Some("52.4")),
    ];
    let mut rendered = String::with_capacity(1024);
    let (_, peak_render) = peak_during(|| {
        template::render_to(template, &values, &mut rendered).unwrap();
    });
    eprintln!("template::render_to peak: {peak_render} B");
    assert_eq!(peak_render, 0, "template::render_to allocated {peak_render} B");
}
