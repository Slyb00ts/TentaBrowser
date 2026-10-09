//! Repeated flushes with mutations must not grow memory without bound.
//!
//! A counting allocator makes the check deterministic: it reports live heap
//! bytes, not resident set size. The test is alone in its process under
//! nextest, so no other test allocates concurrently.
#![cfg(feature = "test-util")]

mod cascade_support;

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

use cascade_support::*;
use tenta_css::cascade::StyleDiff;

struct Counting;

static LIVE: AtomicUsize = AtomicUsize::new(0);

// SAFETY: forwards every call to the system allocator and only counts sizes.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        LIVE.fetch_add(layout.size(), Ordering::Relaxed);
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        LIVE.fetch_add(new_size, Ordering::Relaxed);
        LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

fn live() -> usize {
    LIVE.load(Ordering::Relaxed)
}

#[test]
fn two_hundred_flushes_with_mutations_keep_live_memory_bounded() {
    let cards: String = (0..200)
        .map(|i| {
            format!(
                "<div id=c{i} class='card k{}'><p>text</p><span>x</span></div>",
                i % 7
            )
        })
        .collect();
    let (mut doc, mut engine) = styled(&format!(
        "<!DOCTYPE html><html><head><style>\
         .card {{ padding: 2px }} .k1 > p {{ color: red }} .k2 span {{ margin-left: calc(1px + 1em) }}\
         div:nth-child(3n) {{ width: 10px }} p:not(.x) {{ margin: 1px }}</style></head>\
         <body>{cards}</body></html>"
    ));
    let flush_round = |doc: &mut tenta_dom::Document,
                       engine: &mut tenta_css::cascade::StyleEngine,
                       round: usize| {
        let node = element(doc, &format!("c{}", round % 200));
        set_attribute(doc, node, "class", &format!("card k{}", round % 7));
        set_attribute(doc, node, "style", &format!("width: {}px", round % 5));
        assert_eq!(engine.flush(doc), StyleDiff::Full);
    };
    // Warm-up covers every class/style combination the loop cycles through and
    // fills the caches that may reach a steady size (rule tree, atoms).
    for round in 0..250 {
        flush_round(&mut doc, &mut engine, round);
    }
    let baseline = live();
    for round in 250..450 {
        flush_round(&mut doc, &mut engine, round);
    }
    let growth = live().saturating_sub(baseline);
    assert!(
        growth < 64 * 1024,
        "live heap grew by {growth} bytes over 200 flushes"
    );
}
