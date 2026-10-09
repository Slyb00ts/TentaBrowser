// One real-engine workload shared by the V8 and QuickJS benchmark examples.
// Full checks run before and after each timed batch; the batch checksum keeps
// every result observable without scanning 64 KiB inside the timed loop.
const cloneBufferBytes = 64 * 1024;
const cloneGraphSparseLength = 4097;
const cloneText = String.fromCharCode(0xd800, 0xd83d, 0xde00, 0x00e9);
const cloneLeaf = {text: cloneText, number: 7};
const cloneSparse = [];
cloneSparse.length = cloneGraphSparseLength;
cloneSparse[4096] = cloneLeaf;
const cloneGraphSource = {
  alpha: 1, middle: 2, omega: 3,
  left: cloneLeaf, right: cloneLeaf, sparse: cloneSparse,
};
cloneGraphSource.self = cloneGraphSource;

const cloneCopySource = new ArrayBuffer(cloneBufferBytes);
const cloneCopySourceBytes = new Uint8Array(cloneCopySource);
for (let index = 0; index < cloneBufferBytes; index++) {
  cloneCopySourceBytes[index] = (index * 31 + 7) & 255;
}

let cloneGraphLast = null;
let cloneCopyLast = null;
let cloneTransferLast = null;
let cloneTransferSourceLast = null;

function cloneGraphContribution(value) {
  const text = value.left.text;
  return text.charCodeAt(0) + text.charCodeAt(1) +
    text.charCodeAt(2) + text.charCodeAt(3) +
    value.left.number + value.sparse.length +
    value.alpha + value.middle + value.omega;
}

function verifyCloneGraphShape(value, isSource) {
  if (!value || typeof value !== 'object' ||
      (value === cloneGraphSource) !== isSource ||
      Object.getPrototypeOf(value) !== Object.prototype ||
      Object.keys(value).join(',') !== 'alpha,middle,omega,left,right,sparse,self' ||
      value.alpha !== 1 || value.middle !== 2 || value.omega !== 3 ||
      value.self !== value || !Array.isArray(value.sparse) ||
      value.sparse.length !== cloneGraphSparseLength ||
      Object.keys(value.sparse).join(',') !== '4096' ||
      Object.hasOwn(value.sparse, 0) || Object.hasOwn(value.sparse, 4095) ||
      value.left !== value.right || value.left !== value.sparse[4096] ||
      (value.left === cloneLeaf) !== isSource ||
      Object.keys(value.left).join(',') !== 'text,number' ||
      value.left.text.length !== 4 ||
      value.left.text.charCodeAt(0) !== 0xd800 ||
      value.left.text.charCodeAt(1) !== 0xd83d ||
      value.left.text.charCodeAt(2) !== 0xde00 ||
      value.left.text.charCodeAt(3) !== 0x00e9 ||
      cloneGraphContribution(value) !== 171828) {
    throw new Error('Incorrect cyclic/shared/sparse graph clone');
  }
}

function verifyCloneGraphState() {
  if (cloneGraphLast === null) cloneGraphLast = structuredClone(cloneGraphSource);
  verifyCloneGraphShape(cloneGraphSource, true);
  verifyCloneGraphShape(cloneGraphLast, false);
  return 'ok';
}

function timedGraphClones(count) {
  let checksum = 0;
  for (let index = 0; index < count; index++) {
    const copy = structuredClone(cloneGraphSource);
    cloneGraphLast = copy;
    checksum += cloneGraphContribution(copy);
  }
  return checksum;
}

function verifyCloneCopyState() {
  if (cloneCopyLast === null) cloneCopyLast = structuredClone(cloneCopySource);
  if (cloneCopyLast === cloneCopySource ||
      cloneCopySource.byteLength !== cloneBufferBytes ||
      cloneCopyLast.byteLength !== cloneBufferBytes) {
    throw new Error('Incorrect ArrayBuffer copy identity or length');
  }
  const copied = new Uint8Array(cloneCopyLast);
  let sourceChecksum = 0;
  let copyChecksum = 0;
  for (let index = 0; index < cloneBufferBytes; index++) {
    const expected = (index * 31 + 7) & 255;
    if (cloneCopySourceBytes[index] !== expected || copied[index] !== expected) {
      throw new Error(`Incorrect ArrayBuffer copied byte at ${index}`);
    }
    sourceChecksum += cloneCopySourceBytes[index];
    copyChecksum += copied[index];
  }
  if (sourceChecksum !== 8355840 || copyChecksum !== 8355840) {
    throw new Error('Incorrect full ArrayBuffer copy checksum');
  }
  copied[0] ^= 255;
  if (cloneCopySourceBytes[0] !== 7) {
    throw new Error('ArrayBuffer copy aliases its source');
  }
  copied[0] ^= 255;
  return 'ok';
}

function timedBufferCopies(count) {
  let checksum = 0;
  for (let index = 0; index < count; index++) {
    const copy = structuredClone(cloneCopySource);
    cloneCopyLast = copy;
    const bytes = new Uint8Array(copy);
    checksum += bytes.byteLength + bytes[0] + bytes[31] +
      bytes[32768] + bytes[65535];
  }
  return checksum;
}

function cloneFreshTransferBuffer() {
  const source = new ArrayBuffer(cloneBufferBytes);
  const bytes = new Uint8Array(source);
  bytes[0] = 7;
  bytes[31] = 19;
  bytes[32768] = 23;
  bytes[65535] = 29;
  const copy = structuredClone(source, {transfer: [source]});
  cloneTransferSourceLast = source;
  cloneTransferLast = copy;
  return copy;
}

function verifyCloneTransferState() {
  if (cloneTransferLast === null) cloneFreshTransferBuffer();
  if (cloneTransferSourceLast.byteLength !== 0 ||
      cloneTransferLast === cloneTransferSourceLast ||
      cloneTransferLast.byteLength !== cloneBufferBytes) {
    throw new Error('Incorrect ArrayBuffer transfer identity, length or detachment');
  }
  const bytes = new Uint8Array(cloneTransferLast);
  let checksum = 0;
  for (let index = 0; index < cloneBufferBytes; index++) {
    const expected = index === 0 ? 7 : index === 31 ? 19 :
      index === 32768 ? 23 : index === 65535 ? 29 : 0;
    if (bytes[index] !== expected) {
      throw new Error(`Incorrect transferred ArrayBuffer byte at ${index}`);
    }
    checksum += bytes[index];
  }
  if (checksum !== 78) throw new Error('Incorrect full transfer checksum');
  return 'ok';
}

function timedBufferTransfers(count) {
  let checksum = 0;
  for (let index = 0; index < count; index++) {
    const copy = cloneFreshTransferBuffer();
    const bytes = new Uint8Array(copy);
    checksum += bytes.byteLength + bytes[0] + bytes[31] +
      bytes[32768] + bytes[65535];
  }
  return checksum;
}
