use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::atomic::{AtomicUsize, Ordering};
use zmime::{Limits, Message, Part, decode_encoded_words};

struct Counting;
thread_local! {
    // The allocator can serve unrelated test-runner threads on macOS.
    // Count only allocations made by the thread running this test.
    static TRACK: Cell<bool> = const { Cell::new(false) };
}
static ALLOCATIONS: AtomicUsize = AtomicUsize::new(0);

// SAFETY: all allocation/deallocation requests are forwarded without changing
// pointer, size, alignment, or ownership. Counters are synchronized atomics.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if TRACK.try_with(Cell::get).unwrap_or(false) {
            ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        }
        // SAFETY: forward the caller's valid allocation layout unchanged.
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: ptr came from System above, with the same allocation layout.
        unsafe { System.dealloc(ptr, layout) }
    }
}
#[global_allocator]
static ALLOCATOR: Counting = Counting;

#[test]
fn parsing_unfolding_decoding_and_text_conversion_allocate_nothing() {
    let input = b"Subject: report\r\n continued\r\nContent-Type: multipart/mixed; boundary=x\r\n\r\n\
--x\r\nContent-Type: text/plain; charset=iso-8859-1\r\nContent-Transfer-Encoding: quoted-printable\r\n\r\nCaf=E9\r\n\
--x\r\nContent-Type: application/octet-stream\r\nContent-Disposition: attachment; filename=a.bin\r\nContent-Transfer-Encoding: base64\r\n\r\nAAEC/w==\r\n--x--\r\n";
    let mut parts = [Part::EMPTY; 4];
    let mut bytes = [0u8; 32];
    let mut text = [0u8; 32];
    let mut word = [0u8; 32];
    let mut field = [0u8; 32];
    ALLOCATIONS.store(0, Ordering::Relaxed);
    TRACK.with(|tracking| tracking.set(true));
    let message = Message::parse(input, &mut parts, Limits::DEFAULT).unwrap();
    let header_len = message
        .root()
        .headers()
        .get(b"subject")
        .unwrap()
        .unfolded(&mut field)
        .unwrap();
    let count = message
        .attachments()
        .next()
        .unwrap()
        .decode_body(&mut bytes)
        .unwrap();
    let body = message.parts()[1].text(&mut text).unwrap();
    let word_len = decode_encoded_words(b"=?UTF-8?B?SGVsbG8=?=", &mut word).unwrap();
    TRACK.with(|tracking| tracking.set(false));
    assert_eq!(ALLOCATIONS.load(Ordering::Relaxed), 0);
    assert_eq!(&bytes[..count], &[0, 1, 2, 255]);
    assert_eq!(body, "Café");
    assert_eq!(&word[..word_len], b"Hello");
    assert_eq!(&field[..header_len], b" report continued");
}
