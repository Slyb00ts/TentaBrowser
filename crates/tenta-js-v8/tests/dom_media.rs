//! Media and canvas elements and the SVG interfaces on the V8 host
//! (`tenta_js::bindings::host::media`).

use std::cell::RefCell;
use std::rc::Rc;

use tenta_dom::Document;
use tenta_js_v8::Engine;

fn engine() -> Engine {
    let mut document = Document::new();
    document.set_url(Some(
        "https://example.com/dir/page.html"
            .parse()
            .expect("test URL"),
    ));
    Engine::with_document(Rc::new(RefCell::new(document)))
}

fn eval(engine: &mut Engine, source: &str) -> String {
    engine
        .evaluate(source)
        .expect("test script should evaluate without throwing")
}

/// Each tag's wrapper has its interface in its prototype chain, which is what
/// a page's `instanceof` asks.
#[test]
fn the_interfaces_are_in_the_prototype_chains() {
    let mut engine = engine();
    assert_eq!(
        eval(
            &mut engine,
            "var ns = 'http://www.w3.org/2000/svg';
             var v = document.createElement('video'), a = document.createElement('audio'),
                 c = document.createElement('canvas'), d = document.createElement('div');
             var svg = document.createElementNS(ns, 'svg'), sa = document.createElementNS(ns, 'a'),
                 g = document.createElementNS(ns, 'g'), ha = document.createElement('a');
             [v instanceof HTMLVideoElement, v instanceof HTMLMediaElement, v instanceof HTMLElement,
              a instanceof HTMLAudioElement, a instanceof HTMLMediaElement, a instanceof HTMLVideoElement,
              d instanceof HTMLMediaElement, c instanceof HTMLCanvasElement,
              svg instanceof SVGSVGElement, svg instanceof SVGGraphicsElement, svg instanceof SVGElement,
              sa instanceof SVGAElement, ha instanceof SVGAElement, ha instanceof HTMLAnchorElement,
              g instanceof SVGGraphicsElement, g instanceof SVGElement,
              Object.prototype.toString.call(v), Object.prototype.toString.call(svg),
              Object.getPrototypeOf(HTMLVideoElement) === HTMLMediaElement,
              HTMLMediaElement.HAVE_ENOUGH_DATA, v.NETWORK_NO_SOURCE].join('|')",
        ),
        "true|true|true|true|true|false|false|true|true|true|true|true|false|true|false|true|\
         [object HTMLVideoElement]|[object SVGSVGElement]|true|4|3"
    );
}

/// A media element is one that has loaded nothing: paused, no duration, no
/// type it can play. Its attributes reflect, and the members a script sets to
/// configure it take a value without throwing in strict mode.
#[test]
fn a_media_element_has_loaded_nothing() {
    let mut engine = engine();
    assert_eq!(
        eval(
            &mut engine,
            "'use strict';
             var v = document.createElement('video');
             var before = [v.paused, v.ended, v.seeking, v.readyState, v.networkState, isNaN(v.duration),
                           v.currentTime, v.volume, v.playbackRate, v.muted, v.currentSrc,
                           v.canPlayType('video/mp4'), v.preload, v.width, v.videoWidth].join(',');
             v.src = 'clip.mp4'; v.poster = '/poster.png'; v.width = 640; v.height = 360;
             v.muted = true; v.loop = true; v.autoplay = true; v.controls = true; v.playsInline = true;
             v.volume = 0.5; v.currentTime = 12; v.playbackRate = 2; v.preload = 'none';
             v.pause(); v.load();
             var after = [v.src, v.poster, v.width, v.height, v.muted, v.defaultMuted, v.loop,
                          v.getAttribute('width'), v.hasAttribute('muted'), v.hasAttribute('playsinline'),
                          v.volume, v.currentTime, v.playbackRate, v.preload].join(',');
             before + '|' + after",
        ),
        "true,false,false,0,0,true,0,1,1,false,,,metadata,0,0|\
         https://example.com/dir/clip.mp4,https://example.com/poster.png,640,360,true,true,true,\
         640,true,true,1,0,1,none"
    );
}

/// `play()` answers with a promise and rejects it: there is no source this
/// engine can play.
#[test]
fn play_rejects_with_not_supported() {
    let mut engine = engine();
    eval(
        &mut engine,
        "globalThis.out = 'pending';
         var p = document.createElement('audio').play();
         globalThis.isPromise = p instanceof Promise;
         p.then(function () { out = 'played'; }, function (e) { out = e.name + ': ' + e.message; });
         'started'",
    );
    engine.run_microtasks();
    assert_eq!(
        eval(&mut engine, "isPromise + ' ' + out"),
        "true NotSupportedError: The element has no supported sources."
    );
}

/// A canvas has its dimensions and no context to draw with.
#[test]
fn a_canvas_has_dimensions_and_no_context() {
    let mut engine = engine();
    assert_eq!(
        eval(
            &mut engine,
            "var c = document.createElement('canvas');
             var defaults = [c.width, c.height];
             c.width = 10; c.setAttribute('height', '20');
             [defaults.join('x'), c.width, c.height, c.getAttribute('width'),
              c.getContext('2d'), c.getContext('webgl') === null, c.toDataURL()].join('|')",
        ),
        "300x150|10|20|10||true|data:,"
    );
}
