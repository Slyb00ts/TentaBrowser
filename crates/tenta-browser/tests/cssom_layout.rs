//! What script does to a stylesheet reaches layout (S1): a rule inserted
//! through CSSOM, a property set on a rule's `style`, `sheet.disabled` and a
//! constructed sheet in `adoptedStyleSheets` all change what
//! `getComputedStyle`, element geometry and a screenshot report, although none
//! of them changes the document. Pages are served by the in-process fixture
//! server and run the V8 engine.

#![cfg(all(feature = "render", feature = "v8"))]

use std::io::Cursor;
use std::time::Duration;

use tenta_browser::{
    BrowserContext, ContextOptions, Engine, Goto, JsEngine, Page, PageOptions, ScreenshotOptions,
    Settle, Viewport,
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
    async fn new(files: Vec<(&str, Vec<u8>)>) -> Rig {
        let temp = tenta_testutil::temp_dir("cssom-layout");
        for (name, contents) in files {
            temp.write(name, contents);
        }
        let server = FixtureServer::start(FixtureServerConfig::new(temp.path()))
            .await
            .expect("fixture server");
        let engine = Engine::builder()
            .max_tier(Tier::Render)
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

fn doc(head: &str, body: &str) -> Vec<u8> {
    format!(
        "<!doctype html><html><head><style>html,body{{margin:0}}div{{height:10px}}</style>\
         {head}</head><body>{body}</body></html>"
    )
    .into_bytes()
}

/// `id:width:color` of each listed element, as layout and the cascade have it.
const PROBE: &str = "var probe = (...ids) => ids.map(id => { \
     var e = document.getElementById(id); \
     return id + ':' + e.getBoundingClientRect().width + ':' + getComputedStyle(e).color; \
     }).join(' ');";

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rules_script_inserts_changes_and_deletes_reach_layout() {
    let html = doc(
        "<style id=s>#y{color:rgb(0, 0, 255)}</style>",
        "<div id=x></div><div id=y></div>",
    );
    let rig = Rig::new(vec![("index.html", html)]).await;
    let page = rig.open("/index.html").await;
    assert_eq!(
        text(&page, &format!("{PROBE} probe('x', 'y')")).await,
        "x:400:rgb(0, 0, 0) y:400:rgb(0, 0, 255)"
    );
    // A rule goes in, and a declaration of a rule that was there changes.
    assert_eq!(
        text(
            &page,
            "var sheet = document.getElementById('s').sheet; \
             sheet.insertRule('#x{width:100px;color:rgb(255, 0, 0)}', 0); \
             sheet.cssRules[1].style.color = 'rgb(0, 128, 0)'; \
             probe('x', 'y')"
        )
        .await,
        "x:100:rgb(255, 0, 0) y:400:rgb(0, 128, 0)"
    );
    // An at-rule that applies, one that does not, and a nested rule.
    assert_eq!(
        text(
            &page,
            "sheet.insertRule('@media (min-width: 300px) { #y { width: 50px } }', 2); \
             sheet.insertRule('@media (min-width: 900px) { #y { width: 70px } }', 3); \
             sheet.cssRules[0].insertRule('& { color: rgb(1, 2, 3) }'); \
             probe('x', 'y')"
        )
        .await,
        "x:100:rgb(1, 2, 3) y:50:rgb(0, 128, 0)"
    );
    // A rule goes out; a selector changes. The renamed rule now competes
    // with the later `#y` rule of the same specificity, which wins the color.
    assert_eq!(
        text(
            &page,
            "sheet.deleteRule(2); sheet.cssRules[0].selectorText = '#y'; probe('x', 'y')"
        )
        .await,
        "x:400:rgb(0, 0, 0) y:100:rgb(0, 128, 0)"
    );
    assert_eq!(
        text(&page, "sheet.deleteRule(1); probe('x', 'y')").await,
        "x:400:rgb(0, 0, 0) y:100:rgb(1, 2, 3)"
    );
    // The sheet is turned off and on again: its rules are still the edited ones.
    assert_eq!(
        text(&page, "sheet.disabled = true; probe('x', 'y')").await,
        "x:400:rgb(0, 0, 0) y:400:rgb(0, 0, 0)"
    );
    assert_eq!(
        text(&page, "sheet.disabled = false; probe('x', 'y')").await,
        "x:400:rgb(0, 0, 0) y:100:rgb(1, 2, 3)"
    );
    rig.finish();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_sheet_script_only_reads_or_turns_off_keeps_its_source() {
    // The source has a rule Stylo drops when it parses for script
    // (`@import`), so a sheet rebuilt from the script view would lose it.
    let html = doc(
        "<style id=s>@import 'more.css'; #x{width:100px}</style>",
        "<div id=x></div><div id=y></div>",
    );
    let rig = Rig::new(vec![
        ("index.html", html),
        ("more.css", b"#y{width:60px}".to_vec()),
    ])
    .await;
    let page = rig.open("/index.html").await;
    assert_eq!(
        text(
            &page,
            &format!(
                "{PROBE} var sheet = document.getElementById('s').sheet; \
                 [sheet.cssRules.length, probe('x', 'y')].join(' ')"
            )
        )
        .await,
        "1 x:100:rgb(0, 0, 0) y:60:rgb(0, 0, 0)"
    );
    assert_eq!(
        text(&page, "sheet.disabled = true; probe('x', 'y')").await,
        "x:400:rgb(0, 0, 0) y:400:rgb(0, 0, 0)"
    );
    assert_eq!(
        text(&page, "sheet.disabled = false; probe('x', 'y')").await,
        "x:100:rgb(0, 0, 0) y:60:rgb(0, 0, 0)"
    );
    // Once script edits it, the imports the source began with are kept.
    assert_eq!(
        text(
            &page,
            "sheet.insertRule('#x{color:rgb(9, 9, 9)}', 1); probe('x', 'y')"
        )
        .await,
        "x:100:rgb(9, 9, 9) y:60:rgb(0, 0, 0)"
    );
    rig.finish();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_edit_to_a_linked_sheet_reaches_layout_and_keeps_its_imports() {
    let html = doc(
        "<link id=l rel=stylesheet href=main.css>",
        "<div id=x></div><div id=y></div><div id=z></div>",
    );
    let rig = Rig::new(vec![
        ("index.html", html),
        ("main.css", b"@import 'more.css'; #x{width:100px}".to_vec()),
        ("more.css", b"#y{width:60px}".to_vec()),
    ])
    .await;
    let page = rig.open("/index.html").await;
    assert_eq!(
        text(
            &page,
            &format!(
                "{PROBE} var sheet = document.getElementById('l').sheet; \
                 sheet.insertRule('#z{{width:30px}}', sheet.cssRules.length); \
                 sheet.cssRules[0].style.width = '110px'; \
                 probe('x', 'y', 'z')"
            )
            .replace("{{", "{")
            .replace("}}", "}")
        )
        .await,
        "x:110:rgb(0, 0, 0) y:60:rgb(0, 0, 0) z:30:rgb(0, 0, 0)"
    );
    rig.finish();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn new_text_in_a_style_element_is_a_new_sheet_without_the_old_edits() {
    let html = doc("<style id=s>#x{width:100px}</style>", "<div id=x></div>");
    let rig = Rig::new(vec![("index.html", html)]).await;
    let page = rig.open("/index.html").await;
    assert_eq!(
        text(
            &page,
            &format!(
                "{PROBE} var style = document.getElementById('s'), old = style.sheet; \
                 old.insertRule('#x{{color:rgb(255, 0, 0)}}', 1); probe('x')"
            )
            .replace("{{", "{")
            .replace("}}", "}")
        )
        .await,
        "x:100:rgb(255, 0, 0)"
    );
    // The element's text is replaced while script still holds the old sheet:
    // the old sheet no longer says anything about the element.
    assert_eq!(
        text(
            &page,
            "style.textContent = '#x{width:40px}'; \
             old.insertRule('#x{color:rgb(0, 0, 255)}', 0); \
             [probe('x'), style.sheet === old, style.sheet.cssRules.length].join(' ')"
        )
        .await,
        "x:40:rgb(0, 0, 0) false 1"
    );
    rig.finish();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn adopted_sheets_style_the_document_after_its_own_sheets() {
    let html = doc(
        "<style>#x{width:100px;color:rgb(1, 1, 1)}</style>",
        "<div id=x></div><div id=y></div>",
    );
    let rig = Rig::new(vec![("index.html", html)]).await;
    let page = rig.open("/index.html").await;
    assert_eq!(
        text(
            &page,
            &format!(
                "{PROBE} var first = new CSSStyleSheet(), second = new CSSStyleSheet(); \
                 first.replaceSync('#x{{width:200px}} #y{{width:20px}}'); \
                 second.replaceSync('#y{{width:30px}}'); \
                 var unadopted = probe('x', 'y'); \
                 document.adoptedStyleSheets = [first, second]; \
                 var list = document.adoptedStyleSheets; \
                 [unadopted, '/', probe('x', 'y'), '/', list.length, list[0] === first, \
                  list[1] === second, Object.isFrozen(list)].join(' ')"
            )
            .replace("{{", "{")
            .replace("}}", "}")
        )
        .await,
        "x:400:rgb(0, 0, 0) y:400:rgb(0, 0, 0)".replace("x:400:rgb(0, 0, 0)", "x:100:rgb(1, 1, 1)")
            + " / x:200:rgb(1, 1, 1) y:30:rgb(0, 0, 0) / 2 true true true"
    );
    // An adopted sheet stays live: what script does to it afterwards applies.
    assert_eq!(
        text(
            &page,
            "second.insertRule('#x{color:rgb(2, 2, 2)}'); first.cssRules[0].style.width = '210px'; \
             probe('x', 'y')"
        )
        .await,
        "x:210:rgb(2, 2, 2) y:30:rgb(0, 0, 0)"
    );
    // The order of the list is the order of the cascade, and a sheet that is
    // turned off or taken out of the list stops applying.
    assert_eq!(
        text(
            &page,
            "document.adoptedStyleSheets = [second, first]; var swapped = probe('x', 'y'); \
             first.disabled = true; var off = probe('x', 'y'); \
             document.adoptedStyleSheets = []; \
             [swapped, '/', off, '/', probe('x', 'y'), '/', \
              document.adoptedStyleSheets.length].join(' ')"
        )
        .await,
        "x:210:rgb(2, 2, 2) y:20:rgb(0, 0, 0) / x:100:rgb(2, 2, 2) y:30:rgb(0, 0, 0) / \
         x:100:rgb(1, 1, 1) y:400:rgb(0, 0, 0) / 0"
    );
    rig.finish();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_shadow_root_adopts_sheets_for_its_own_tree() {
    let html = doc(
        "",
        "<div id=host><template shadowrootmode=open><div id=inner></div></template></div>\
         <div id=outer></div>",
    );
    let rig = Rig::new(vec![("index.html", html)]).await;
    let page = rig.open("/index.html").await;
    assert_eq!(
        text(
            &page,
            "var root = document.getElementById('host').shadowRoot; \
             var width = e => e.getBoundingClientRect().width; \
             var sheet = new CSSStyleSheet(); \
             sheet.replaceSync('div{width:70px} :host{width:300px}'); \
             root.adoptedStyleSheets = [sheet]; \
             var adopted = [width(root.getElementById('inner')), \
               width(document.getElementById('outer')), width(document.getElementById('host'))]; \
             root.adoptedStyleSheets = []; \
             [adopted.join(','), width(root.getElementById('inner')), \
              document.adoptedStyleSheets.length, root.adoptedStyleSheets.length].join(' ')"
        )
        .await,
        "70,400,300 400 0 0"
    );
    rig.finish();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn only_a_sheet_constructed_in_the_document_can_be_adopted() {
    let html = doc("<style id=s>#x{width:100px}</style>", "<div id=x></div>");
    let rig = Rig::new(vec![("index.html", html)]).await;
    let page = rig.open("/index.html").await;
    assert_eq!(
        text(
            &page,
            "var good = new CSSStyleSheet(); document.adoptedStyleSheets = [good]; \
             var attempt = value => { try { document.adoptedStyleSheets = value; return 'ok'; } \
               catch (error) { return error.name; } }; \
             var pushed; try { document.adoptedStyleSheets.push(good); pushed = 'ok'; } \
               catch (error) { pushed = error.name; } \
             [attempt([document.getElementById('s').sheet]), attempt([good, {}]), attempt(7), \
              pushed, document.adoptedStyleSheets.length, \
              document.adoptedStyleSheets[0] === good].join(' ')"
        )
        .await,
        "NotAllowedError TypeError TypeError TypeError 1 true"
    );
    rig.finish();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_screenshot_shows_what_script_put_in_a_sheet() {
    let html = doc("<style id=s></style>", "<div id=x></div>");
    let rig = Rig::new(vec![("index.html", html)]).await;
    let page = rig.open("/index.html").await;
    text(
        &page,
        "document.getElementById('s').sheet.insertRule( \
           '#x{width:100px;height:100px;background:rgb(255, 0, 0)}'); \
         var sheet = new CSSStyleSheet(); \
         sheet.replaceSync('body{background:rgb(0, 0, 255)}'); \
         document.adoptedStyleSheets = [sheet]; 'done'",
    )
    .await;
    let shot = page
        .screenshot(ScreenshotOptions::default())
        .await
        .expect("screenshot");
    let mut reader = png::Decoder::new(Cursor::new(&shot.bytes))
        .read_info()
        .expect("a decodable PNG");
    let mut rgba = vec![0; reader.output_buffer_size().expect("a sized buffer")];
    let info = reader.next_frame(&mut rgba).expect("a frame");
    assert_eq!(info.color_type, png::ColorType::Rgba);
    let pixel = |x: u32, y: u32| -> [u8; 4] {
        let at = ((y * info.width + x) * 4) as usize;
        rgba[at..at + 4].try_into().expect("four channels")
    };
    assert_eq!(pixel(50, 50), [255, 0, 0, 255], "the inserted rule's box");
    assert_eq!(
        pixel(250, 150),
        [0, 0, 255, 255],
        "the adopted sheet's page"
    );
    rig.finish();
}
