const CSS_GRAPH_RULES = 64;

function setupCssWrapperGraph() {
    const sheet = new CSSStyleSheet();
    let source = "";
    for (let index = 0; index < CSS_GRAPH_RULES; index++) {
        source += `.c${index}{color:red}`;
    }
    sheet.replaceSync(source);
    globalThis.cssGraphSheet = sheet;
    let checksum = 0;
    for (let index = 0; index < CSS_GRAPH_RULES; index++) {
        const rule = sheet.cssRules[index];
        rule.style.note = index;
        checksum += rule.style.getPropertyValue("color").length;
    }
    return checksum;
}

function readCssWrapperGraph(calls) {
    let checksum = 0;
    for (let index = 0; index < calls; index++) {
        const rule = cssGraphSheet.cssRules[index % CSS_GRAPH_RULES];
        checksum += rule.style.getPropertyValue("color").length;
    }
    return checksum;
}

function churnCssWrapperGraph(pairs) {
    let checksum = 0;
    for (let index = 0; index < pairs; index++) {
        const at = cssGraphSheet.cssRules.length;
        cssGraphSheet.insertRule(`.temporary${index}{color:blue}`, at);
        const removed = cssGraphSheet.cssRules[at];
        checksum += removed.style.getPropertyValue("color").length;
        cssGraphSheet.deleteRule(at);
    }
    return checksum;
}

function verifyCssWrapperGraph() {
    if (cssGraphSheet.cssRules.length !== CSS_GRAPH_RULES) return "wrong rule count";
    for (let index = 0; index < CSS_GRAPH_RULES; index++) {
        if (cssGraphSheet.cssRules[index].style.getPropertyValue("color") !== "red") {
            return "wrong style value";
        }
    }
    return "ok";
}
