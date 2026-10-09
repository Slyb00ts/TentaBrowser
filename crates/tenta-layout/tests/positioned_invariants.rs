//! Structural invariants of positioned layout over generated trees: every
//! element with a box has a fragment reachable from the root, an absolutely
//! positioned box with insets and a size lands where its containing block
//! puts it, and laying out twice gives the same fragments.

#![cfg(feature = "embedded-fonts")]

mod support;

use tenta_layout::{FragId, FragmentTree, Rect};

/// A small deterministic generator, so a failing tree can be reproduced.
struct Lcg(u64);

impl Lcg {
    fn next(&mut self, bound: u64) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (self.0 >> 33) % bound
    }
}

struct Node {
    id: usize,
    style: String,
    abs: bool,
    children: Vec<Node>,
}

fn generate(rng: &mut Lcg, next_id: &mut usize, depth: u32) -> Node {
    let id = *next_id;
    *next_id += 1;
    let position = [
        "static", "relative", "absolute", "absolute", "fixed", "sticky",
    ][rng.next(6) as usize];
    let display =
        ["block", "block", "flex", "grid", "inline-block", "inline"][rng.next(6) as usize];
    let mut style = format!("position:{position};");
    if matches!(position, "absolute" | "fixed") {
        style.push_str(&format!(
            "left:{}px;top:{}px;width:{}px;height:{}px;",
            rng.next(40),
            rng.next(40),
            10 + rng.next(30),
            10 + rng.next(30)
        ));
    } else {
        style.push_str(&format!(
            "display:{display};width:{}px;min-height:{}px;margin:{}px;",
            40 + rng.next(80),
            5 + rng.next(20),
            rng.next(6)
        ));
        if position == "relative" && rng.next(2) == 0 {
            style.push_str("left:3px;top:2px;");
        }
    }
    if rng.next(5) == 0 {
        style.push_str("overflow:hidden;");
    }
    if rng.next(5) == 0 {
        style.push_str("transform:translate(0,0);");
    }
    let abs = matches!(position, "absolute" | "fixed");
    let count = if depth == 0 { 0 } else { rng.next(4) };
    let children = (0..count)
        .map(|_| generate(rng, next_id, depth - 1))
        .collect();
    Node {
        id,
        style,
        abs,
        children,
    }
}

fn render(node: &Node, out: &mut String) {
    out.push_str(&format!("<div id=n{} style=\"{}\">", node.id, node.style));
    if !node.abs {
        out.push_str("text ");
    }
    for child in &node.children {
        render(child, out);
    }
    out.push_str("</div>");
}

fn reaches_root(frags: &FragmentTree, mut frag: FragId) -> bool {
    while let Some(parent) = frags.parent(frag) {
        frag = parent;
    }
    frag == frags.root
}

fn snapshot(frags: &FragmentTree) -> Vec<(Rect, usize)> {
    (0..frags.len() as u32)
        .map(|i| {
            (
                frags.get(FragId(i)).rect,
                frags.get(FragId(i)).children.len(),
            )
        })
        .collect()
}

#[test]
fn generated_trees_keep_every_box_reachable_and_deterministic() {
    for seed in 0..60u64 {
        let mut rng = Lcg(seed + 1);
        let mut next_id = 0;
        let tree = generate(&mut rng, &mut next_id, 4);
        let mut html = String::from(
            "<!doctype html><style>body{margin:0;font:14px 'Liberation Sans'}</style><body>",
        );
        render(&tree, &mut html);
        let first = support::lay_out(&html, (800.0, 600.0));
        let second = support::lay_out(&html, (800.0, 600.0));
        assert_eq!(
            snapshot(&first.frags),
            snapshot(&second.frags),
            "seed {seed}"
        );
        for id in 0..next_id {
            let name = format!("n{id}");
            let node = first.node(&name);
            let frags = first.frags.fragments_of(node);
            if frags.is_empty() {
                continue;
            }
            assert!(
                frags.iter().all(|&f| reaches_root(&first.frags, f)),
                "seed {seed}: #{name} is cut off from the root\n{html}"
            );
        }
    }
}

#[test]
fn an_absolute_box_with_insets_sits_inside_its_positioned_ancestor() {
    for seed in 0..40u64 {
        let mut rng = Lcg(1000 + seed);
        let (l, t) = (rng.next(30), rng.next(30));
        let (w, h) = (10 + rng.next(30), 10 + rng.next(30));
        let (pl, pt, pb) = (rng.next(20), rng.next(20), rng.next(5));
        let html = format!(
            "<!doctype html><style>body{{margin:0}}</style><body>\
             <div style=\"height:{pt}px\"></div>\
             <div id=cb style=\"position:relative;margin-left:{pl}px;border:{pb}px solid;\
             padding:3px;width:200px;height:150px\">\
             <div><div id=a style=\"position:absolute;left:{l}px;top:{t}px;width:{w}px;height:{h}px\">\
             </div></div></div>"
        );
        let laid = support::lay_out(&html, (800.0, 600.0));
        let cb = laid.rect("cb");
        let a = laid.rect("a");
        let (origin_x, origin_y) = (cb.x + pb as f32, cb.y + pb as f32);
        assert_eq!(
            (a.x, a.y, a.w, a.h),
            (origin_x + l as f32, origin_y + t as f32, w as f32, h as f32),
            "seed {seed}"
        );
    }
}
