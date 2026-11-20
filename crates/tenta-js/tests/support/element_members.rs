// Included by V8 and QuickJS adapters: reflected members of elements and
// documents that have no behaviour of their own in a headless engine, and the
// namespace lookups of `Node`.

use std::cell::RefCell;
use std::rc::Rc;

use tenta_dom::Document;

fn eval(source: &str) -> String {
    let mut document = Document::new();
    document.set_url(Some(
        "https://example.com/dir/page.html".parse().expect("test URL"),
    ));
    let mut engine = new_engine(Rc::new(RefCell::new(document)));
    engine.evaluate(source).expect("element member script")
}

#[test]
fn html_elements_reflect_their_global_attributes() {
    assert_eq!(
        eval(
            "const el=document.createElement('div'); \
             const before=[el.draggable,el.spellcheck,el.translate,el.contentEditable, \
               el.isContentEditable,el.inputMode,el.enterKeyHint,el.autocapitalize,el.autofocus, \
               el.nonce,el.popover].join(','); \
             el.draggable=true;el.spellcheck=false;el.translate=false;el.inputMode='Numeric'; \
             el.enterKeyHint='search';el.autocapitalize='words';el.autofocus=true;el.nonce='n1'; \
             el.popover='manual'; \
             const after=[el.draggable,el.getAttribute('draggable'),el.spellcheck, \
               el.getAttribute('spellcheck'),el.translate,el.getAttribute('translate'), \
               el.inputMode,el.getAttribute('inputmode'),el.enterKeyHint,el.autocapitalize, \
               el.autofocus,el.hasAttribute('autofocus'),el.nonce,el.popover].join(','); \
             el.popover=null;el.draggable=false; \
             const input=document.createElement('input');input.autofocus=true; \
             [before,after,el.hasAttribute('popover'),el.getAttribute('draggable'), \
              input.autofocus,input.hasAttribute('autofocus'), \
              'draggable' in HTMLElement.prototype].join('|')"
        ),
        "false,true,true,inherit,false,,,,false,,\
         |true,true,false,false,false,no,numeric,Numeric,search,words,true,true,n1,manual\
         |false|false|true|true|true"
    );
}

#[test]
fn content_editable_is_inherited_and_takes_only_its_keywords() {
    assert_eq!(
        eval(
            "const attempt=f=>{try{return String(f())}catch(e){return e.name}}; \
             const outer=document.createElement('div'),inner=document.createElement('span'), \
               off=document.createElement('b'); \
             outer.append(inner);inner.append(off); \
             outer.contentEditable='true';off.contentEditable='false'; \
             const plain=document.createElement('p');plain.setAttribute('contenteditable',''); \
             const only=document.createElement('p');only.contentEditable='PLAINTEXT-ONLY'; \
             const states=[outer.isContentEditable,inner.isContentEditable,inner.contentEditable, \
               off.isContentEditable,off.contentEditable,plain.contentEditable, \
               plain.isContentEditable,only.contentEditable,only.isContentEditable].join(','); \
             outer.contentEditable='inherit'; \
             [states,outer.hasAttribute('contenteditable'),inner.isContentEditable, \
              attempt(()=>{outer.contentEditable='yes'}), \
              outer.outerText===outer.innerText].join('|')"
        ),
        "true,true,inherit,false,false,true,true,plaintext-only,true|false|false|SyntaxError|true"
    );
}

#[test]
fn aria_attributes_reflect_as_nullable_strings() {
    assert_eq!(
        eval(
            "const el=document.createElement('div'),svg=document.createElementNS( \
               'http://www.w3.org/2000/svg','svg'); \
             const before=[el.role,el.ariaLabel,el.ariaHidden,el.ariaValueNow].join(','); \
             el.role='button';el.ariaLabel='Close';el.ariaExpanded='false'; \
             el.setAttribute('aria-hidden','true');el.ariaValueNow=5;svg.ariaLabel='chart'; \
             const set=[el.getAttribute('role'),el.getAttribute('aria-label'),el.ariaExpanded, \
               el.ariaHidden,el.getAttribute('aria-valuenow'),svg.getAttribute('aria-label')] \
               .join(','); \
             el.ariaLabel=null;el.role=null; \
             [before,set,el.hasAttribute('aria-label'),el.hasAttribute('role'), \
              ['ariaChecked','ariaSelected','ariaDisabled','ariaPressed','ariaCurrent','ariaLive', \
               'ariaBusy','ariaAtomic','ariaDescription','ariaHasPopup','ariaModal','ariaRequired', \
               'ariaReadOnly','ariaPlaceholder','ariaValueMin','ariaValueMax','ariaValueText', \
               'ariaLevel','ariaSort','ariaOrientation','ariaMultiSelectable','ariaMultiLine', \
               'ariaAutoComplete','ariaColCount','ariaColIndex','ariaColSpan','ariaRowCount', \
               'ariaRowIndex','ariaRowSpan','ariaPosInSet','ariaSetSize','ariaKeyShortcuts', \
               'ariaRoleDescription','ariaInvalid'].filter(n=>!(n in Element.prototype)).length] \
               .join('|')"
        ),
        ",,,|button,Close,false,true,5,chart|false|false|0"
    );
}

#[test]
fn a_document_answers_for_its_direction_editing_and_fullscreen() {
    assert_eq!(
        eval(
            "const html=document.createElement('html');document.appendChild(html); \
             const before=[document.dir,document.designMode].join(','); \
             document.dir='RTL';document.designMode='ON'; \
             const set=[document.dir,html.getAttribute('dir'),html.dir,document.designMode].join(','); \
             document.designMode='nonsense';document.dir='sideways'; \
             [before,set,document.designMode,document.dir,document.execCommand('copy'), \
              document.queryCommandSupported('copy'),document.queryCommandEnabled('bold'), \
              /^\\d\\d\\/\\d\\d\\/\\d{4} \\d\\d:\\d\\d:\\d\\d$/.test(document.lastModified), \
              document.fullscreenElement,document.fullscreenEnabled, \
              document.pictureInPictureEnabled,document.pointerLockElement].join('|')"
        ),
        ",off|rtl,RTL,rtl,on|on||false|false|false|true||false|false|"
    );
}

#[test]
fn get_html_and_set_html_unsafe_are_inner_html() {
    assert_eq!(
        eval(
            "const el=document.createElement('div');el.setHTMLUnsafe('<p id=a>one</p><b>two</b>'); \
             [el.children.length,el.firstChild.id,el.getHTML(),el.getHTML()===el.innerHTML] \
               .join('|')"
        ),
        "2|a|<p id=\"a\">one</p><b>two</b>|true"
    );
}

#[test]
fn nodes_look_up_namespaces_and_prefixes() {
    assert_eq!(
        eval(
            "const XHTML='http://www.w3.org/1999/xhtml',SVG='http://www.w3.org/2000/svg', \
               XLINK='http://www.w3.org/1999/xlink',XMLNS='http://www.w3.org/2000/xmlns/'; \
             const html=document.createElement('html');document.appendChild(html); \
             const svg=document.createElementNS(SVG,'svg');html.appendChild(svg); \
             svg.setAttributeNS(XMLNS,'xmlns:xlink',XLINK); \
             const use=document.createElementNS(SVG,'use');svg.appendChild(use); \
             const text=document.createTextNode('t');use.appendChild(text); \
             const pre=document.createElementNS('urn:x','x:item');html.appendChild(pre); \
             const loose=document.createTextNode('loose'),fragment=document.createDocumentFragment(); \
             [html.lookupNamespaceURI(null),html.isDefaultNamespace(XHTML),html.lookupPrefix(XHTML), \
              use.lookupNamespaceURI(null),use.lookupNamespaceURI('xlink'),use.lookupPrefix(XLINK), \
              text.lookupNamespaceURI('xlink'),text.isDefaultNamespace(SVG), \
              use.lookupNamespaceURI('nope'),use.lookupNamespaceURI(''),use.lookupPrefix(''), \
              use.lookupPrefix(null),pre.lookupNamespaceURI('x'),pre.lookupPrefix('urn:x'), \
              pre.isDefaultNamespace('urn:x'),document.lookupNamespaceURI(null), \
              document.isDefaultNamespace(XHTML),loose.lookupNamespaceURI(null), \
              loose.isDefaultNamespace(null),fragment.lookupPrefix(XHTML), \
              html.lookupNamespaceURI('xml'),html.lookupNamespaceURI('xmlns')].join('|')"
        ),
        "http://www.w3.org/1999/xhtml|true||http://www.w3.org/2000/svg\
         |http://www.w3.org/1999/xlink|xlink|http://www.w3.org/1999/xlink|true||\
         http://www.w3.org/2000/svg|||urn:x|x|false|http://www.w3.org/1999/xhtml|true||true|\
         |http://www.w3.org/XML/1998/namespace|http://www.w3.org/2000/xmlns/"
    );
}
