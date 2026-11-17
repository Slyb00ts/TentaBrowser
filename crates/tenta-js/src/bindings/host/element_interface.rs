//! Which interface an HTML element has
//! (<https://html.spec.whatwg.org/multipage/dom.html#elements-in-the-dom>, the
//! *element interface* of a name in the HTML namespace).
//!
//! Both engines' wrapper worlds ask this when they make an element's wrapper,
//! so the table is here once.

/// The interface of the HTML-namespace element named `tag`.
///
/// A name the HTML Standard defines has the interface the standard gives it,
/// which is `HTMLElement` itself for the elements with no interface of their
/// own (`section`, `em`, `code` and the like). Any other name is
/// `HTMLElement` when it contains a hyphen — the shape of a custom element
/// name — and `HTMLUnknownElement` otherwise. The standard tests for a
/// *valid* custom element name, which also excludes eight reserved hyphenated
/// names (`annotation-xml`, `font-face` and six more); those read as
/// `HTMLElement` here.
pub fn html_element_interface(tag: &str) -> &'static str {
    match tag {
        "iframe" => "HTMLIFrameElement",
        "template" => "HTMLTemplateElement",
        "div" => "HTMLDivElement",
        "span" => "HTMLSpanElement",
        "p" => "HTMLParagraphElement",
        "a" => "HTMLAnchorElement",
        "img" => "HTMLImageElement",
        "label" => "HTMLLabelElement",
        "form" => "HTMLFormElement",
        "input" => "HTMLInputElement",
        "button" => "HTMLButtonElement",
        "select" => "HTMLSelectElement",
        "textarea" => "HTMLTextAreaElement",
        "option" => "HTMLOptionElement",
        "script" => "HTMLScriptElement",
        "link" => "HTMLLinkElement",
        "style" => "HTMLStyleElement",
        "video" => "HTMLVideoElement",
        "audio" => "HTMLAudioElement",
        "canvas" => "HTMLCanvasElement",
        "slot" => "HTMLSlotElement",
        "meta" => "HTMLMetaElement",
        "source" => "HTMLSourceElement",
        "dialog" => "HTMLDialogElement",
        "html" => "HTMLHtmlElement",
        "head" => "HTMLHeadElement",
        "title" => "HTMLTitleElement",
        "base" => "HTMLBaseElement",
        "body" => "HTMLBodyElement",
        "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => "HTMLHeadingElement",
        "hr" => "HTMLHRElement",
        "pre" | "listing" | "xmp" => "HTMLPreElement",
        "blockquote" | "q" => "HTMLQuoteElement",
        "ol" => "HTMLOListElement",
        "ul" => "HTMLUListElement",
        "menu" => "HTMLMenuElement",
        "li" => "HTMLLIElement",
        "dl" => "HTMLDListElement",
        "data" => "HTMLDataElement",
        "time" => "HTMLTimeElement",
        "br" => "HTMLBRElement",
        "ins" | "del" => "HTMLModElement",
        "picture" => "HTMLPictureElement",
        "embed" => "HTMLEmbedElement",
        "object" => "HTMLObjectElement",
        "track" => "HTMLTrackElement",
        "map" => "HTMLMapElement",
        "area" => "HTMLAreaElement",
        "table" => "HTMLTableElement",
        "caption" => "HTMLTableCaptionElement",
        "col" | "colgroup" => "HTMLTableColElement",
        "thead" | "tbody" | "tfoot" => "HTMLTableSectionElement",
        "tr" => "HTMLTableRowElement",
        "td" | "th" => "HTMLTableCellElement",
        "fieldset" => "HTMLFieldSetElement",
        "legend" => "HTMLLegendElement",
        "optgroup" => "HTMLOptGroupElement",
        "datalist" => "HTMLDataListElement",
        "output" => "HTMLOutputElement",
        "progress" => "HTMLProgressElement",
        "meter" => "HTMLMeterElement",
        "details" => "HTMLDetailsElement",
        "param" => "HTMLParamElement",
        "font" => "HTMLFontElement",
        "dir" => "HTMLDirectoryElement",
        "frame" => "HTMLFrameElement",
        "frameset" => "HTMLFrameSetElement",
        "marquee" => "HTMLMarqueeElement",
        "abbr" | "address" | "article" | "aside" | "b" | "bdi" | "bdo" | "cite" | "code" | "dd"
        | "dfn" | "dt" | "em" | "figcaption" | "figure" | "footer" | "header" | "hgroup" | "i"
        | "kbd" | "main" | "mark" | "nav" | "noscript" | "rp" | "rt" | "ruby" | "s" | "samp"
        | "search" | "section" | "small" | "strong" | "sub" | "summary" | "sup" | "u" | "var"
        | "wbr" | "acronym" | "basefont" | "big" | "center" | "nobr" | "noembed" | "noframes"
        | "plaintext" | "rb" | "rtc" | "strike" | "tt" => "HTMLElement",
        name if name.contains('-') => "HTMLElement",
        _ => "HTMLUnknownElement",
    }
}

#[cfg(test)]
mod tests {
    use super::html_element_interface;

    #[test]
    fn a_name_has_the_interface_the_standard_gives_it() {
        for (tag, interface) in [
            ("div", "HTMLDivElement"),
            ("h3", "HTMLHeadingElement"),
            ("body", "HTMLBodyElement"),
            ("td", "HTMLTableCellElement"),
            ("th", "HTMLTableCellElement"),
            ("tbody", "HTMLTableSectionElement"),
            ("colgroup", "HTMLTableColElement"),
            ("del", "HTMLModElement"),
            ("q", "HTMLQuoteElement"),
            ("section", "HTMLElement"),
            ("summary", "HTMLElement"),
            ("my-widget", "HTMLElement"),
            ("blink", "HTMLUnknownElement"),
            ("foo", "HTMLUnknownElement"),
        ] {
            assert_eq!(html_element_interface(tag), interface, "{tag}");
        }
    }
}
