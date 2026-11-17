//! Element geometry, `getComputedStyle` and hit testing as script sees them
//! (CSS1): the page's layout answers `getBoundingClientRect`, `offsetWidth`,
//! `clientHeight`, `scrollHeight`, `elementFromPoint` and the resolved values of
//! `getComputedStyle`. Pages are served by the in-process fixture server and run
//! the V8 engine.

#![cfg(all(feature = "render", feature = "v8"))]

use std::time::Duration;

use tenta_browser::{
    BrowserContext, ContextOptions, Engine, Goto, JsEngine, Page, PageOptions, Settle, Viewport,
};
use tenta_core::{FsPolicy, Tier};
use tenta_testutil::{FixtureServer, FixtureServerConfig, TempDir};

const PATIENCE: Duration = Duration::from_secs(60);

struct Rig {
    server: FixtureServer,
    context: BrowserContext,
    engine: Engine,
    _files: TempDir,
}

impl Rig {
    async fn new(files: Vec<(&str, Vec<u8>)>, tier: Tier) -> Rig {
        let temp = tenta_testutil::temp_dir("geometry-js");
        for (name, contents) in files {
            temp.write(name, contents);
        }
        let server = FixtureServer::start(FixtureServerConfig::new(temp.path()))
            .await
            .expect("fixture server");
        let engine = Engine::builder()
            .max_tier(tier)
            .io_threads(1)
            .page_threads(1)
            .fs_policy(FsPolicy::none().allow_read(temp.path()))
            .js(JsEngine::V8)
            .ephemeral(true)
            .build()
            .expect("engine");
        let context = engine
            .new_context(ContextOptions {
                allow_private_network: true,
                ..ContextOptions::default()
            })
            .expect("context");
        Rig {
            server,
            context,
            engine,
            _files: temp,
        }
    }

    async fn open(&self, path: &str) -> Page {
        let page = self
            .context
            .new_page(PageOptions {
                javascript: true,
                viewport: Viewport {
                    width: 400,
                    height: 300,
                    device_scale_factor: 1.0,
                    mobile: false,
                },
                ..PageOptions::default()
            })
            .await
            .expect("page");
        page.goto(
            &self.server.url(path),
            Goto::default().wait_until(Settle::Load).timeout(PATIENCE),
        )
        .await
        .expect("navigation");
        page
    }

    fn finish(self) {
        self.context.close();
        self.engine.close();
    }
}

async fn text(page: &Page, expression: &str) -> String {
    page.evaluate(expression)
        .await
        .expect("ran")
        .into_value::<String>()
        .expect("a string")
}

fn doc(style: &str, body: &str) -> Vec<u8> {
    format!(
        "<!doctype html><html><head><style>html,body{{margin:0}}{style}</style></head>\
         <body>{body}</body></html>"
    )
    .into_bytes()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn block_geometry_matches_the_layout() {
    let html = doc(
        "#a{width:120px;height:30px;margin:10px 0 0 20px;padding:5px;border:2px solid red}\
         #b{position:absolute;left:50px;top:100px;width:60px;height:40px}\
         #c{height:900px}",
        "<div id=a>box</div><div id=b></div><div id=c></div>",
    );
    let rig = Rig::new(vec![("index.html", html)], Tier::Render).await;
    let page = rig.open("/index.html").await;
    assert_eq!(
        text(
            &page,
            "var a = document.getElementById('a'), r = a.getBoundingClientRect();\
             [r.x, r.y, r.width, r.height, r.top, r.right, r.bottom, r.left, \
              a.offsetWidth, a.offsetHeight, a.offsetTop, a.offsetLeft, \
              a.clientWidth, a.clientHeight, a.clientTop, a.clientLeft, \
              a.scrollWidth, a.scrollHeight, a.getClientRects().length, \
              r instanceof DOMRect, r instanceof DOMRectReadOnly].join(',')"
        )
        .await,
        "20,10,134,44,10,154,54,20,134,44,10,20,130,40,2,2,130,40,1,true,true"
    );
    assert_eq!(
        text(
            &page,
            "var b = document.getElementById('b');\
             [b.offsetParent === document.body, b.offsetTop, b.offsetLeft, \
              document.documentElement.clientWidth, document.documentElement.clientHeight, \
              document.documentElement.scrollHeight, document.body.offsetParent].join(',')"
        )
        .await,
        "true,100,50,400,300,954,"
    );
    rig.finish();
}

/// A component with a shadow tree, as script sees it: computed styles come
/// from the sheet inside the shadow root, the boxes are where the flat tree
/// puts them, a sheet added later applies, and a point inside the component
/// is the host's from the document's side.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_shadow_tree_has_styles_and_geometry_of_its_own() {
    let html = doc(
        "p{color:rgb(1, 0, 0)}",
        "<div id=host><template shadowrootmode=open>\
         <style>:host{display:block;width:200px;padding:10px}\
         #row{display:flex;height:40px}#box{width:60px}\
         p{color:rgb(0, 2, 0);margin:0;line-height:20px}\
         ::slotted(span){display:block;width:30px}</style>\
         <div id=row><div id=box></div><slot></slot></div><p id=note>note</p>\
         </template><span id=light></span></div><p id=outer>outer</p>",
    );
    let rig = Rig::new(vec![("index.html", html)], Tier::Render).await;
    let page = rig.open("/index.html").await;
    let rect = "var rect = e => { var r = e.getBoundingClientRect(); \
                return [r.x, r.y, r.width, r.height].join(','); };";
    assert_eq!(
        text(
            &page,
            &format!(
                "{rect} var root = document.getElementById('host').shadowRoot; \
                 [getComputedStyle(root.getElementById('note')).color, \
                  getComputedStyle(document.getElementById('outer')).color, \
                  rect(document.getElementById('host')), \
                  rect(root.getElementById('box')), \
                  rect(document.getElementById('light')), \
                  document.elementFromPoint(20, 20).id].join(' | ')"
            )
        )
        .await,
        "rgb(0, 2, 0) | rgb(1, 0, 0) | 0,0,220,80 | 10,10,60,40 | 70,10,30,40 | host"
    );
    // A sheet script adds to the shadow root afterwards applies there too.
    assert_eq!(
        text(
            &page,
            "var extra = document.createElement('style'); \
             extra.textContent = '#box{width:90px}'; root.appendChild(extra); \
             rect(root.getElementById('box')) + ' | ' + rect(document.getElementById('light'))"
        )
        .await,
        "10,10,90,40 | 100,10,30,40"
    );
    rig.finish();
}
