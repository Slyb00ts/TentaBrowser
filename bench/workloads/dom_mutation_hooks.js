"use strict";

function setupDomMutationHooks() {
    const html = document.createElement("html");
    const body = document.createElement("body");
    const connected = document.createElement("div");
    const detached = document.createElement("div");
    const connectedText = document.createTextNode("seed");
    const detachedText = document.createTextNode("seed");
    const connectedChild = document.createElement("span");
    const detachedChild = document.createElement("span");

    html.appendChild(body);
    document.appendChild(html);
    body.appendChild(connected);
    connected.appendChild(connectedText);
    detached.appendChild(detachedText);
    globalThis.domMutationHooks = {
        html, body, connected, detached,
        connectedText, detachedText, connectedChild, detachedChild,
    };
    return "ready";
}

function runDomMutationHooks(mode, count) {
    if (!Number.isInteger(count) || count <= 0 || count % 2 !== 0) {
        throw new Error("the mutation count must be a positive even integer");
    }
    const state = globalThis.domMutationHooks;
    const parent = mode === "connected" ? state.connected
        : mode === "detached" ? state.detached : null;
    if (parent === null) throw new Error("unknown mutation mode");
    const text = mode === "connected" ? state.connectedText : state.detachedText;
    const child = mode === "connected" ? state.connectedChild : state.detachedChild;
    let checksum = 0;
    for (let index = 0; index < count; index++) {
        const value = (index & 1) === 0 ? "a" : "bb";
        parent.setAttribute("data-bench", value);
        const observed = parent.getAttribute("data-bench");
        if (observed !== value) throw new Error("attribute set lost its value");
        parent.removeAttribute("data-bench");
        if (parent.hasAttribute("data-bench")) throw new Error("attribute removal failed");

        text.data = value;
        if (text.data !== value) throw new Error("character data replacement failed");

        parent.appendChild(child);
        if (child.parentNode !== parent) throw new Error("child insertion failed");
        checksum += observed.length + text.length + parent.childNodes.length;
        parent.removeChild(child);
        if (child.parentNode !== null) throw new Error("child removal failed");
    }
    return checksum;
}

function verifyDomMutationHooks() {
    const state = globalThis.domMutationHooks;
    if (document.documentElement !== state.html || state.html.parentNode !== document) {
        return "connected root changed";
    }
    if (state.connected.parentNode !== state.body || state.detached.parentNode !== null) {
        return "connection state changed";
    }
    for (const [parent, text, child] of [
        [state.connected, state.connectedText, state.connectedChild],
        [state.detached, state.detachedText, state.detachedChild],
    ]) {
        if (parent.hasAttribute("data-bench") || text.data !== "bb") {
            return "attribute or text state changed";
        }
        if (parent.childNodes.length !== 1 || parent.firstChild !== text) {
            return "permanent text child changed";
        }
        if (child.parentNode !== null) return "temporary child remained attached";
    }
    return "ok";
}

function cleanupDomMutationHooks() {
    const state = globalThis.domMutationHooks;
    document.removeChild(state.html);
    delete globalThis.domMutationHooks;
    return "ok";
}
