//! Tests that the Chromium nesting clamp reaches timers scheduled *by script*.
//!
//! The clamp (`docs/spec/05-javascript.md` §5) raises a `setTimeout` deeper
//! than nesting level 5 to a 4 ms floor, which is what stops a
//! `setTimeout(f, 0)` recursion from starving the loop. The depth has to
//! survive a detour: the task that dequeues a fired timer only moves the
//! handler onto a ready queue, and the JS call happens after that task has
//! already restored the depth. Unless the depth travels with the handler,
//! every script-scheduled timer looks like depth 1 and the clamp never fires.

use tenta_js_v8::Engine;

/// Times the engine's virtual clock reports as a chain of zero-delay timers
/// recurses. `performance.now()` reads the same loop clock the timer queue
/// uses, so the gaps are exactly the scheduled delays.
fn zero_delay_chain_times(depth: usize) -> Vec<f64> {
    let mut engine = Engine::new();
    engine
        .evaluate(&format!(
            r#"
            globalThis.times = [];
            function step() {{
                times.push(performance.now());
                if (times.length < {depth}) {{ setTimeout(step, 0); }}
            }}
            setTimeout(step, 0);
        "#
        ))
        .expect("script evaluates");
    engine.run_event_loop(64);
    engine
        .evaluate("globalThis.times.join(',')")
        .expect("times read back")
        .split(',')
        .map(|value| value.parse().expect("a number"))
        .collect()
}

#[test]
fn a_script_scheduled_zero_timeout_chain_clamps_once_it_nests_past_five() {
    let times = zero_delay_chain_times(9);
    assert_eq!(times.len(), 9, "the whole chain ran");

    let gaps: Vec<f64> = times.windows(2).map(|pair| pair[1] - pair[0]).collect();
    // Levels 2..=5 are still inside the limit and stay at the requested 0 ms;
    // from level 6 on every hop is raised to the 4 ms floor.
    let (unclamped, clamped) = gaps.split_at(4);
    assert!(
        unclamped.iter().all(|gap| *gap < 4.0),
        "hops within the nesting limit must not be clamped, got {unclamped:?}"
    );
    assert!(
        clamped.iter().all(|gap| *gap >= 4.0),
        "hops past the nesting limit must be raised to the 4 ms floor, got {clamped:?}"
    );
}

#[test]
fn the_clamp_does_not_leak_into_a_fresh_top_level_timer() {
    // Nesting is a property of the chain, not of the loop: a timer scheduled
    // from top-level script after a deep chain starts again at depth 1.
    let mut engine = Engine::new();
    engine
        .evaluate(
            r#"
            globalThis.deep = 0;
            function spin() { deep += 1; if (deep < 8) { setTimeout(spin, 0); } }
            setTimeout(spin, 0);
        "#,
        )
        .expect("script evaluates");
    engine.run_event_loop(64);

    engine
        .evaluate(
            r#"
            globalThis.started = performance.now();
            globalThis.fresh = null;
            setTimeout(() => { globalThis.fresh = performance.now() - started; }, 0);
        "#,
        )
        .expect("script evaluates");
    engine.run_event_loop(8);

    let gap: f64 = engine
        .evaluate("String(globalThis.fresh)")
        .expect("gap read back")
        .parse()
        .expect("a number");
    assert!(
        gap < 4.0,
        "a top-level timer starts at nesting depth 1, got a {gap} ms gap"
    );
}
