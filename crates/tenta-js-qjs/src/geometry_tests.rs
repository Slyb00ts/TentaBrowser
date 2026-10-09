//! The window metrics read the page viewport until a geometry host is
//! installed, then the host.

use std::rc::Rc;
use std::time::Duration;

use tenta_core::NodeId;
use tenta_js::geometry::{
    ClientMetrics, GeometryHost, OffsetMetrics, RectF64, ScrollMetrics, ViewportMetrics,
};

use tenta_js::engine::{EngineClock, PageEngine, PageEngineConfig, PageEngineFactory};
use tenta_js::storage::PageStorage;
use tenta_net::CookieJar;

use super::QuickJsEngineFactory;

/// Reports the given `innerWidth` and fixed values for everything else.
struct FakeGeometry(f64);

impl GeometryHost for FakeGeometry {
    fn viewport(&self) -> ViewportMetrics {
        ViewportMetrics {
            inner_width: self.0,
            inner_height: 555.0,
            outer_width: 800.0,
            outer_height: 600.0,
            device_pixel_ratio: 2.0,
            scroll_x: 11.0,
            scroll_y: 40.0,
        }
    }
    fn bounding_rect(&self, _: NodeId) -> Option<RectF64> {
        None
    }
    fn client_rects(&self, _: NodeId) -> Vec<RectF64> {
        Vec::new()
    }
    fn offset_metrics(&self, _: NodeId) -> Option<OffsetMetrics> {
        None
    }
    fn client_metrics(&self, _: NodeId) -> Option<ClientMetrics> {
        None
    }
    fn scroll_metrics(&self, _: NodeId) -> Option<ScrollMetrics> {
        None
    }
    fn computed_value(&self, _: NodeId, _: Option<&str>, _: &str) -> Option<String> {
        None
    }
    fn computed_property_names(&self) -> &'static [&'static str] {
        &[]
    }
    fn element_from_point(&self, _: f64, _: f64) -> Option<NodeId> {
        None
    }
}

const METRICS: &str = "[innerWidth, innerHeight, outerWidth, outerHeight, devicePixelRatio, \
    screenX, screenY, scrollX, scrollY, pageXOffset, pageYOffset].join(',')";
const FROM_HOST: &str = "777,555,800,600,2,0,0,11,40,11,40";
const REPLACE: &str = "'use strict'; window.innerWidth = 5; \
    [innerWidth, innerHeight, Object.keys(window).includes('innerWidth')].join(',')";
const DESCRIPTOR: &str = "var d = Object.getOwnPropertyDescriptor(window, 'innerWidth'); \
    [typeof d.get, typeof d.set, d.enumerable, d.configurable].join(',')";
const ILLEGAL: &str = "var d = Object.getOwnPropertyDescriptor(window, 'innerWidth'); \
    var out = []; \
    try { d.get.call({}); } catch (e) { out.push(e instanceof TypeError); } \
    try { d.set.call({}, 1); } catch (e) { out.push(e instanceof TypeError); } \
    out.join(',')";
const FROM_PAGE_VIEWPORT: &str = "640,480,640,480,1.5,0,0,0,0,0,0";

fn factory_engine(viewport: ViewportMetrics) -> Box<dyn PageEngine> {
    let config = PageEngineConfig {
        timer_rule: tenta_core::TimerRule::default(),
        clock: EngineClock::Virtual,
        max_heap_bytes: None,
        script_timeout: None,
        locale: "en-US".to_owned(),
        timezone: None,
        cookies: CookieJar::new(),
        storage: PageStorage::isolated(),
        native_memory: tenta_core::NativeMemoryBudget::for_page(
            tenta_core::Budgets::platform_default().page_memory_mb,
        ),
        viewport,
    };
    QuickJsEngineFactory
        .create(
            Rc::new(std::cell::RefCell::new(tenta_dom::Document::new())),
            &config,
        )
        .expect("the QuickJS factory builds an engine")
}

fn eval(engine: &mut Box<dyn PageEngine>, source: &str) -> String {
    engine
        .evaluate(source, Duration::from_secs(5))
        .expect("the script runs")
}

#[test]
fn window_metrics_follow_the_page_viewport_until_a_geometry_host_is_installed() {
    let mut engine = factory_engine(ViewportMetrics {
        inner_width: 640.0,
        inner_height: 480.0,
        outer_width: 640.0,
        outer_height: 480.0,
        device_pixel_ratio: 1.5,
        scroll_x: 0.0,
        scroll_y: 0.0,
    });
    assert_eq!(eval(&mut engine, METRICS), FROM_PAGE_VIEWPORT);
    engine.set_geometry_host(Rc::new(FakeGeometry(777.0)));
    assert_eq!(eval(&mut engine, METRICS), FROM_HOST);
}

#[test]
fn a_geometry_host_installed_twice_is_replaced() {
    let mut engine = factory_engine(ViewportMetrics::default());
    engine.set_geometry_host(Rc::new(FakeGeometry(777.0)));
    engine.set_geometry_host(Rc::new(FakeGeometry(9.0)));
    assert_eq!(eval(&mut engine, "innerWidth"), "9");
}

#[test]
fn assigning_a_window_metric_replaces_it_and_leaves_the_others() {
    let mut engine = factory_engine(ViewportMetrics::default());
    engine.set_geometry_host(Rc::new(FakeGeometry(777.0)));
    assert_eq!(eval(&mut engine, REPLACE), "5,555,true");
}

#[test]
fn window_metrics_are_replaceable_accessors() {
    let mut engine = factory_engine(ViewportMetrics::default());
    assert_eq!(eval(&mut engine, DESCRIPTOR), "function,function,true,true");
    assert_eq!(eval(&mut engine, ILLEGAL), "true,true");
}
