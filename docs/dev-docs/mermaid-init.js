// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
//
// adapted from the file `mdbook-mermaid install` generates. the upstream
// version binds to theme buttons by bare id (`ayu`, `navy`, ...), which mdbook
// 0.5 renamed to `mdbook-theme-*`, so every listener threw a TypeError and
// diagrams kept the light palette after a theme switch. it also predates the
// "Auto" theme that follows the os preference. this version watches the class
// list on <html> instead, which covers the picker, Auto, and another tab
// changing the stored theme, and re-renders in place rather than reloading.
//
// re-apply these changes if you refresh the file with `mdbook-mermaid install`.

(() => {
    const darkThemes = ['ayu', 'navy', 'coal'];

    const themeOf = () => {
        for (const cssClass of document.documentElement.classList) {
            if (darkThemes.includes(cssClass)) {
                return 'dark';
            }
        }
        return 'default';
    };

    // mermaid replaces each block's text with an <svg>, so keep the original
    // source around to re-render from when the theme changes
    const sources = new Map();
    let seq = 0;

    const collect = () => {
        for (const el of document.querySelectorAll('.mermaid')) {
            if (el.dataset.mermaidId === undefined) {
                const id = String(seq++);
                el.dataset.mermaidId = id;
                sources.set(id, el.textContent);
            }
        }
    };

    const render = async () => {
        for (const el of document.querySelectorAll('.mermaid')) {
            const source = sources.get(el.dataset.mermaidId);
            if (source === undefined) {
                continue;
            }
            el.textContent = source;
            el.removeAttribute('data-processed');
        }

        mermaid.initialize({ startOnLoad: false, theme: themeOf() });

        try {
            await mermaid.run({ querySelector: '.mermaid' });
        } catch (err) {
            // a malformed diagram must not take the rest of the page with it
            console.error('mermaid failed to render a diagram', err);
        }
    };

    const start = () => {
        collect();
        render();

        let current = themeOf();
        new MutationObserver(() => {
            const next = themeOf();
            if (next !== current) {
                current = next;
                render();
            }
        }).observe(document.documentElement, {
            attributes: true,
            attributeFilter: ['class'],
        });
    };

    if (document.readyState === 'loading') {
        document.addEventListener('DOMContentLoaded', start);
    } else {
        start();
    }
})();
