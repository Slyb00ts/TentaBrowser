//! Golden tests for the extractors on small hand-written documents
//! (`docs/spec/04-dom-and-css.md` §5).

mod support;

use tenta_dom::extract;

const ARTICLE: &str = r#"<!doctype html>
<html lang="en">
<head>
  <title>Extraction sample</title>
  <meta name="description" content="A small page.">
  <meta property="og:title" content="Sample (OG)">
  <meta name="twitter:card" content="summary">
  <link rel="canonical" href="/canonical">
  <script type="application/ld+json">{"@type":"Article","headline":"Sample"}</script>
</head>
<body>
<article itemscope itemtype="https://schema.org/Article">
  <h1 itemprop="headline">Sample <em>title</em></h1>
  <p>Intro with a <a href="/one" rel="nofollow">link</a> and <strong>bold</strong> and <code>code()</code>.</p>
  <h2>Details</h2>
  <ul><li>first</li><li>second<ul><li>nested</li></ul></li></ul>
  <ol><li>step one</li><li>step two</li></ol>
  <blockquote><p>Quoted text.</p></blockquote>
  <pre class="language-rust"><code>fn main() {}</code></pre>
  <figure><img src="/pic.png" alt="A picture"><figcaption>Caption</figcaption></figure>
  <table>
    <tr><th>Name</th><th>Value</th></tr>
    <tr><td>a</td><td>1</td></tr>
    <tr><td>b</td><td>2</td></tr>
  </table>
  <hr>
  <p itemprop="author">Written by someone</p>
</article>
</body>
</html>"#;

fn article() -> tenta_dom::Document {
    support::parse_with_url(ARTICLE, "https://example.test/docs/page.html")
}

#[test]
fn markdown_golden() {
    let document = article();
    let body = document.body().expect("body");
    let markdown = extract::markdown(&document, body);
    let expected = "\
# Sample *title*

Intro with a [link](https://example.test/one) and **bold** and `code()`.

## Details

- first

- second

  - nested

1. step one

2. step two

> Quoted text.

```rust
fn main() {}
```

![A picture](https://example.test/pic.png)

Caption

| Name | Value |
| --- | --- |
| a | 1 |
| b | 2 |

---

Written by someone
";
    assert_eq!(markdown, expected, "\n--- got ---\n{markdown}");
}

#[test]
fn metadata_golden() {
    let document = article();
    let metadata = document.metadata();
    assert_eq!(metadata.title.as_deref(), Some("Extraction sample"));
    assert_eq!(metadata.lang.as_deref(), Some("en"));
    assert_eq!(metadata.meta.get("description").unwrap(), "A small page.");
    assert_eq!(metadata.open_graph.get("title").unwrap(), "Sample (OG)");
    assert_eq!(metadata.twitter.get("card").unwrap(), "summary");
    assert_eq!(
        metadata.canonical.as_deref(),
        Some("https://example.test/canonical")
    );
    assert_eq!(metadata.description(), Some("A small page."));
    assert_eq!(metadata.json_ld.len(), 1);
    assert_eq!(metadata.json_ld[0]["headline"], "Sample");

    assert_eq!(metadata.microdata.len(), 1);
    let item = &metadata.microdata[0];
    assert_eq!(
        item.item_type.as_deref(),
        Some("https://schema.org/Article")
    );
    assert_eq!(item.properties["headline"], vec!["Sample title".to_owned()]);
    assert_eq!(
        item.properties["author"],
        vec!["Written by someone".to_owned()]
    );
}

#[test]
fn links_golden() {
    let document = article();
    let links = document.links();
    assert_eq!(links.len(), 1);
    assert_eq!(links[0].url, "https://example.test/one");
    assert_eq!(links[0].text, "link");
    assert_eq!(links[0].rel.as_deref(), Some("nofollow"));
}

const FORM: &str = r#"<html><body>
<form id="signup" name="signup" action="/register" method="post" enctype="multipart/form-data">
  <label for="email">Email address</label>
  <input id="email" name="email" type="email" required placeholder="you@example.test">
  <label>Password <input name="password" type="password"></label>
  <select name="plan">
    <option value="free">Free</option>
    <option value="pro" selected>Pro</option>
  </select>
  <textarea name="bio">About me</textarea>
  <input name="hidden_field" type="hidden" value="42">
  <button type="submit">Join</button>
</form>
<input name="outside" form="signup" value="associated">
</body></html>"#;

#[test]
fn forms_golden() {
    let document = support::parse_with_url(FORM, "https://example.test/a/b.html");
    let forms = document.forms();
    assert_eq!(forms.len(), 1);
    let form = &forms[0];
    assert_eq!(form.id.as_deref(), Some("signup"));
    assert_eq!(form.name.as_deref(), Some("signup"));
    assert_eq!(
        form.action.as_deref(),
        Some("https://example.test/register")
    );
    assert_eq!(form.method, "POST");
    assert_eq!(form.enctype.as_deref(), Some("multipart/form-data"));

    let summary: Vec<(&str, &str, Option<&str>)> = form
        .fields
        .iter()
        .map(|field| {
            (
                field.name.as_deref().unwrap_or(""),
                field.field_type.as_str(),
                field.label.as_deref(),
            )
        })
        .collect();
    assert_eq!(
        summary,
        vec![
            ("email", "email", Some("Email address")),
            ("password", "password", Some("Password")),
            ("plan", "select", None),
            ("bio", "textarea", None),
            ("hidden_field", "hidden", None),
            ("", "submit", None),
            ("outside", "text", None),
        ]
    );

    let email = &form.fields[0];
    assert!(email.required);
    assert_eq!(email.placeholder.as_deref(), Some("you@example.test"));

    let plan = &form.fields[2];
    assert_eq!(plan.options.len(), 2);
    assert_eq!(plan.options[1].value, "pro");
    assert_eq!(plan.options[1].label, "Pro");
    assert!(plan.options[1].selected);

    assert_eq!(form.fields[3].value.as_deref(), Some("About me"));
    assert_eq!(form.fields[6].value.as_deref(), Some("associated"));
}

const SNAPSHOT_PAGE: &str = r#"<html><body>
<nav aria-label="Primary"><a href="/home">Home</a></nav>
<main>
  <h2>Section title</h2>
  <p>Body text.</p>
  <img src="/x.png" alt="Diagram">
  <button disabled>Send</button>
  <input type="checkbox" checked aria-label="Agree">
  <details open><summary>More</summary><p>Hidden by default</p></details>
  <div role="alert">Careful</div>
  <span hidden>invisible to the snapshot</span>
</main>
</body></html>"#;

#[test]
fn snapshot_golden() {
    let document = support::parse_with_url(SNAPSHOT_PAGE, "https://example.test/");
    let main = document
        .query_selector(document.document_node(), "main")
        .unwrap()
        .expect("main");
    let snapshot = extract::snapshot(&document, main);
    let text = snapshot.to_text();
    let expected = "\
main
  heading \"Section title\" level=2
    text \"Section title\"
  paragraph
    text \"Body text.\"
  image \"Diagram\"
  button \"Send\" disabled=true
    text \"Send\"
  checkbox \"Agree\" checked=true
  group expanded=true
    button \"More\"
      text \"More\"
    paragraph
      text \"Hidden by default\"
  alert
    text \"Careful\"
";
    assert_eq!(text, expected, "\n--- got ---\n{text}");

    let json = snapshot.to_json();
    assert_eq!(json["role"], "main");
    assert_eq!(json["children"][0]["role"], "heading");
    assert_eq!(json["children"][0]["level"], 2);
    assert_eq!(json["children"][3]["disabled"], true);
}

#[test]
fn snapshot_names_links_and_labels() {
    let document = support::parse_with_url(SNAPSHOT_PAGE, "https://example.test/");
    let nav = document
        .query_selector(document.document_node(), "nav")
        .unwrap()
        .expect("nav");
    let snapshot = extract::snapshot(&document, nav);
    assert_eq!(snapshot.role, "navigation");
    assert_eq!(snapshot.name, "Primary", "aria-label wins over content");
    let link = &snapshot.children[0];
    assert_eq!(link.role, "link");
    assert_eq!(link.name, "Home");
    assert_eq!(link.state["url"], "https://example.test/home");
}

#[test]
fn main_content_prefers_the_article_over_navigation() {
    let document = support::parse_with_url(
        "<html><body>\
         <nav><a href=/1>one</a><a href=/2>two</a><a href=/3>three</a><a href=/4>four</a></nav>\
         <div id=\"content\"><p>This is a real paragraph of prose, with commas, several of them, \
         and enough length to be scored as content rather than as navigation chrome.</p>\
         <p>A second paragraph, also long enough to matter, keeps the container ahead of the \
         list of links in the sidebar.</p></div>\
         </body></html>",
        "https://example.test/",
    );
    let main = document.main_content().expect("content");
    assert_eq!(document.element_id(main.node), Some("content"));
    assert!(main.text.starts_with("This is a real paragraph"));
    assert_eq!(main.link_density, 0.0);
}
