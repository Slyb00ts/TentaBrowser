
        globalThis.benchElement = document.createElement('div');
        benchElement.setAttribute('id', 'bench-id');
        globalThis.benchHas = function (count) {
            let matches = 0;
            for (let i = 0; i < count; i++) {
                if (benchElement.hasAttribute('id')) matches++;
            }
            return matches;
        };
        globalThis.benchGet = function (count) {
            let length = 0;
            for (let i = 0; i < count; i++) {
                const value = benchElement.getAttribute('id');
                if (value !== 'bench-id') throw new Error('unexpected id');
                length += value.length;
            }
            return length;
        };
