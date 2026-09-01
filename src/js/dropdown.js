// dropdown.js — custom dropdown to replace native <select>. A native
// select's popup list is rendered by the OS/webview, not the page, so it
// ignores our CSS theme variables entirely on most engines (confirmed
// unreadable — white background regardless of theme — on WebKitGTK).
// This is plain styled DOM instead, fully themeable.
//
// Two modes:
//  - Value-select (default): trigger label shows the current selection,
//    used for Role/log-level/map-tag style pickers.
//  - staticLabel: trigger label never changes (e.g. "+ filter by tag"),
//    used where picking an item performs an action rather than setting
//    a persisted single value.
function mountDropdown(container, { items, value = '', placeholder = 'Select…', staticLabel = null, onSelect }) {
    container.classList.add('astryx-dropdown');
    container.innerHTML = `
        <div class="astryx-dropdown-trigger" tabindex="0">
            <span class="astryx-dropdown-label"></span>
            <span class="astryx-dropdown-arrow">▾</span>
        </div>
        <div class="astryx-dropdown-menu"></div>
    `;
    const trigger = container.querySelector('.astryx-dropdown-trigger');
    const label = container.querySelector('.astryx-dropdown-label');
    const menu = container.querySelector('.astryx-dropdown-menu');

    function currentLabelFor(val) {
        if (staticLabel) return staticLabel;
        const item = items.find(it => it.value === val);
        return item ? item.label : placeholder;
    }

    function renderItems(list) {
        items = list;
        menu.innerHTML = list.length
            ? list.map(it => `<div class="astryx-dropdown-item" data-value="${escapeHtml(it.value)}">${escapeHtml(it.label)}</div>`).join('')
            : `<div class="astryx-dropdown-item astryx-dropdown-empty">Nothing here yet</div>`;
        menu.querySelectorAll('[data-value]').forEach(el => {
            el.addEventListener('click', () => {
                const val = el.dataset.value;
                container.classList.remove('open');
                if (!staticLabel) label.textContent = currentLabelFor(val);
                if (onSelect) onSelect(val);
            });
        });
    }

    renderItems(items);
    label.textContent = currentLabelFor(value);

    trigger.addEventListener('click', () => container.classList.toggle('open'));
    trigger.addEventListener('keydown', (e) => { if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); container.classList.toggle('open'); } });
    document.addEventListener('click', (e) => { if (!container.contains(e.target)) container.classList.remove('open'); });

    return {
        setItems: renderItems,
        setValue: (val) => { label.textContent = currentLabelFor(val); },
    };
}
