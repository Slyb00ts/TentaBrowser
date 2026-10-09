// Shared real-engine checks for qualified element names across document types.

#[test]
fn element_names_uppercase_only_in_html_documents_and_namespaces() {
    let mut engine = new_engine();
    assert_eq!(
        engine
            .evaluate(
                r#"
        const htmlNamespace = 'http://www.w3.org/1999/xhtml';
        const svgNamespace = 'http://www.w3.org/2000/svg';
        const xml = new Document();
        const parsed = new DOMParser().parseFromString(
            '<h:MiXeD xmlns:h="http://www.w3.org/1999/xhtml"/>',
            'application/xhtml+xml');
        const cases = [
            [xml.createElementNS(htmlNamespace, 'p:MiXeD'), 'p:MiXeD'],
            [xml.createElementNS(htmlNamespace, 'MiXeD'), 'MiXeD'],
            [parsed.documentElement, 'h:MiXeD'],
            [document.createElementNS(htmlNamespace, 'p:MiXeD'), 'P:MIXED'],
            [document.createElementNS(htmlNamespace, 'p:éCase'), 'P:éCASE'],
            [document.createElementNS(svgNamespace, 'p:clipPath'), 'p:clipPath'],
            [document.createElementNS(null, 'MiXeD'), 'MiXeD']
        ];
        cases.map(([element, expected]) =>
            [element.tagName === expected, element.nodeName === expected].join(','))
            .join('|');
    "#
            )
            .expect("qualified element names"),
        "true,true|true,true|true,true|true,true|true,true|true,true|true,true"
    );
}
