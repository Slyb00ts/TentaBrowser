// Shared real-engine regression for generated pair-iterator array publication.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use tenta_dom::Document;
use tenta_js::engine::PageEngine;

#[test]
fn pair_iterators_define_own_array_indices_without_calling_page_setters() {
    let mut engine = new_engine(Rc::new(RefCell::new(Document::new())));
    let result = PageEngine::evaluate(
        &mut engine,
        r#"(() => {
            const headers = new Headers([['a', 'one'], ['b', 'two']]);
            const form = new FormData();
            form.append('a', 'one');
            form.append('b', 'two');
            const inputs = [headers, form];
            const methods = ['entries', 'keys', 'values'];
            let hits = 0;
            let valid = true;
            Object.defineProperty(Array.prototype, '0', {
                set(_) { ++hits }, configurable: true
            });
            Object.defineProperty(Array.prototype, '1', {
                set(_) { ++hits }, configurable: true
            });
            try {
                for (const input of inputs) {
                    for (const method of methods) {
                        const iterator = input[method]();
                        const first = iterator.next();
                        const second = iterator.next();
                        valid = valid && !first.done && !second.done && iterator.next().done;
                        if (method === 'entries') {
                            for (const entry of [first.value, second.value]) {
                                valid = valid && Array.isArray(entry);
                                if (!Array.isArray(entry)) continue;
                                const a = Object.getOwnPropertyDescriptor(entry, '0');
                                const b = Object.getOwnPropertyDescriptor(entry, '1');
                                valid = valid && !!a && !!b && a.writable && a.enumerable && a.configurable
                                    && b.writable && b.enumerable && b.configurable;
                            }
                            valid = valid && !!first.value && !!second.value
                                && first.value[0] === 'a' && first.value[1] === 'one'
                                && second.value[0] === 'b' && second.value[1] === 'two';
                        } else if (method === 'keys') {
                            valid = valid && first.value === 'a' && second.value === 'b';
                        } else {
                            valid = valid && first.value === 'one' && second.value === 'two';
                        }
                    }
                }
                return valid && hits === 0;
            } finally {
                delete Array.prototype[0];
                delete Array.prototype[1];
            }
        })()"#,
        Duration::from_secs(10),
    )
    .expect("pair iterator publication");
    assert_eq!(result, "true");
    assert_eq!(engine.native_failure(), None);
}

#[test]
fn form_data_overloads_preserve_strings_blobs_and_optional_filenames() {
    let mut engine = new_engine(Rc::new(RefCell::new(Document::new())));
    let result = PageEngine::evaluate(
        &mut engine,
        r#"(() => {
            for (const method of ['append', 'set']) {
                const form = new FormData();
                form[method]('text', 'value');
                if (form.get('text') !== 'value') return false;
                const file = new File(['abc'], 'original.txt');
                form[method]('file', file);
                if (form.get('file') !== file) return false;
                form[method]('renamed', file, 'renamed.txt');
                const renamed = form.get('renamed');
                if (renamed === file || renamed.name !== 'renamed.txt' || renamed.size !== 3) return false;
                let extraCoercions = 0;
                form[method]('extra', file, 'extra.txt', {
                    toString() { ++extraCoercions; throw new Error('unused argument'); }
                });
                if (extraCoercions !== 0 || form.get('extra').name !== 'extra.txt') return false;
                form[method]('blob', new Blob(['xyz']));
                const blob = form.get('blob');
                if (!(blob instanceof File) || blob.name !== 'blob' || blob.size !== 3) return false;
                let rejected = false;
                try { form[method]('invalid', 'text', 'filename.txt'); }
                catch (error) { rejected = error instanceof TypeError; }
                if (!rejected || form.has('invalid')) return false;
            }
            return true;
        })()"#,
        Duration::from_secs(10),
    )
    .expect("FormData overload conversion");
    assert_eq!(result, "true");
    assert_eq!(engine.native_failure(), None);
}
