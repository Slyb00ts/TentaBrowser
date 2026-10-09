// Shared real-engine workload; no page network requests are made.
const storageKeys = Array.from({length: 128}, (_, index) => `key_${index}`);
const storageArea = localStorage;
const storagePersistentSymbol = Symbol('persistent');
const storageTransientSymbol = Symbol('transient');
storageArea[storagePersistentSymbol] = 17;

function resetStorage() {
    storageArea.clear();
    for (const key of storageKeys) storageArea.setItem(key, 'a');
}

function storageGetItem(count) {
    let checksum = 0;
    for (let index = 0; index < count; index++) {
        checksum += storageArea.getItem(storageKeys[index % 128]).length;
    }
    return checksum;
}

function storageNamedGet(count) {
    let checksum = 0;
    for (let index = 0; index < count; index++) {
        checksum += storageArea[storageKeys[index % 128]].length;
    }
    return checksum;
}

function storageSetItem(count) {
    for (let index = 0; index < count; index++) {
        storageArea.setItem(storageKeys[index % 128], (index & 128) === 0 ? 'b' : 'a');
    }
    return count;
}

function storageNamedSet(count) {
    for (let index = 0; index < count; index++) {
        storageArea[storageKeys[index % 128]] = (index & 128) === 0 ? 'b' : 'a';
    }
    return count;
}

// Enumeration timing checks membership separately; existing ordering regressions
// remain correctness gates and may fail in a preserved baseline executable.
function storageOwnKeys(count) {
    let checksum = 0;
    for (let index = 0; index < count; index++) {
        checksum += Reflect.ownKeys(storageArea).length;
    }
    return checksum / 129;
}

function storageSymbolCreateDelete(count) {
    for (let index = 0; index < count; index++) {
        storageArea[storageTransientSymbol] = index;
        delete storageArea[storageTransientSymbol];
    }
    return count;
}

function verifyStorage(count, wrote) {
    if (storageArea.length !== 128) throw new Error('Unexpected storage length');
    const keys = Reflect.ownKeys(storageArea);
    if (keys.length !== 129 || !keys.includes(storagePersistentSymbol) ||
        storageKeys.some(key => !keys.includes(key)) ||
        storageArea[storagePersistentSymbol] !== 17 ||
        Object.hasOwn(storageArea, storageTransientSymbol)) {
        throw new Error('Incorrect Storage own property membership');
    }
    storageArea[storageTransientSymbol] = 23;
    const temporary = Object.getOwnPropertyDescriptor(storageArea, storageTransientSymbol);
    if (!temporary || temporary.value !== 23 || !temporary.writable ||
        !temporary.enumerable || !temporary.configurable ||
        !Reflect.deleteProperty(storageArea, storageTransientSymbol)) {
        throw new Error('Incorrect ordinary Symbol property lifecycle');
    }
    for (let key = 0; key < 128; key++) {
        const lastWrite = key + Math.floor((count - 1 - key) / 128) * 128;
        const expected = wrote && lastWrite >= 0 && (lastWrite & 128) === 0 ? 'b' : 'a';
        if (storageArea.getItem(storageKeys[key]) !== expected ||
            storageArea[storageKeys[key]] !== expected) {
            throw new Error(`Incorrect stored value at key ${key}`);
        }
    }
    return 'ok';
}
