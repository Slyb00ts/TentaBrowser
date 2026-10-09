// Shared real-engine listener ownership workload for V8 and QuickJS.
function setupConnected(count) {
    globalThis.listenerGcFired = 0;
    const root = document.createElement('div');
    document.appendChild(root);
    for (let index = 0; index < count; index++) {
        const node = document.createElement('span');
        root.appendChild(node);
        node.addEventListener('probe', () => {
            if (node.nodeName === 'SPAN') globalThis.listenerGcFired++;
        });
    }
}

function verifyConnected(count) {
    let dispatched = 0;
    for (let node = document.documentElement.firstChild; node; node = node.nextSibling) {
        node.dispatchEvent(new Event('probe'));
        dispatched++;
    }
    if (dispatched !== count || globalThis.listenerGcFired !== count) {
        throw Error(`connected listeners: dispatched ${dispatched}, fired ${globalThis.listenerGcFired}`);
    }
    return globalThis.listenerGcFired;
}

function setupSelfCapture(count) {
    for (let index = 0; index < count; index++) {
        const node = document.createElement('div');
        node.addEventListener('probe', () => node.nodeName);
    }
}

function setupCrossCapture(pairCount) {
    for (let index = 0; index < pairCount; index++) {
        const first = document.createElement('div');
        const second = document.createElement('span');
        first.addEventListener('probe', () => second.nodeName);
        second.addEventListener('probe', () => first.nodeName);
    }
}

function setupReachableDetached(count) {
    globalThis.listenerGcFired = 0;
    globalThis.listenerGcHeld = [];
    for (let index = 0; index < count; index++) {
        const node = document.createElement('div');
        node.addEventListener('probe', () => {
            if (node.nodeName === 'DIV') globalThis.listenerGcFired++;
        });
        listenerGcHeld.push(node);
    }
}

function verifyReachableDetached(count) {
    for (const node of listenerGcHeld) node.dispatchEvent(new Event('probe'));
    if (globalThis.listenerGcHeld.length !== count || globalThis.listenerGcFired !== count) {
        throw Error(`reachable listeners: held ${globalThis.listenerGcHeld.length}, fired ${globalThis.listenerGcFired}`);
    }
    return globalThis.listenerGcFired;
}

function listenerGcNoop() {}

function setupNoncapturing(count) {
    for (let index = 0; index < count; index++) {
        const node = document.createElement('div');
        node.addEventListener('probe', listenerGcNoop);
    }
}
