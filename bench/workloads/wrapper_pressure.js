function runWrapperPressure(shape, iterations) {
    const root = document.appendChild(document.createElement("div"));
    globalThis.pressureRoot = root;
    root.note = "retained";
    let checksum = 0;
    if (shape === "detached") {
        for (let i = 0; i < iterations; i++) {
            const node = root.appendChild(document.createElement("b"));
            node.note = {owner: node, stamp: i, data: Array(32).fill(i)};
            checksum += node.note.stamp;
            node.remove();
        }
    } else if (shape === "owners") {
        for (let i = 0; i < iterations; i++) {
            const node = document.createElement("b");
            node.style.note = {owner: node, stamp: i};
            node.attributes.note = {owner: node, stamp: i};
            node.classList.note = {owner: node, stamp: i};
            if (node.style.note.stamp !== i || node.attributes.note.stamp !== i ||
                node.classList.note.stamp !== i) {
                throw new Error("owner wrapper state was lost");
            }
            checksum += i;
        }
    } else if (shape === "connected") {
        for (let i = 0; i < iterations; i++) {
            root.appendChild(document.createElement("b")).note = i;
        }
        let count = 0;
        for (let node = root.firstChild; node; node = node.nextSibling) {
            if (node.note !== count++) throw new Error("connected wrapper state was lost");
            checksum += node.note;
        }
        if (count !== iterations) throw new Error("connected node count changed");
    } else if (shape === "deep") {
        let tail = root;
        for (let i = 0; i < iterations; i++) {
            tail = tail.appendChild(document.createElement("b"));
            tail.note = i;
            tail.style.note = i;
        }
        let count = 0;
        for (let node = root.firstChild; node; node = node.firstChild) {
            if (node.note !== count || node.style.note !== count++) {
                throw new Error("deep wrapper state was lost");
            }
            checksum += node.note;
        }
        if (count !== iterations) throw new Error("deep node count changed");
    } else {
        throw new Error("unknown pressure shape");
    }
    if (root.note !== "retained") throw new Error("root wrapper state was lost");
    return checksum;
}
