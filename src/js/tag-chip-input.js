// tag-chip-input.js — mounts an "add tag" chip input into `container`.
// onAdd(tagName) is called when the user commits a tag (Enter or picking
// a suggestion). Tags can contain spaces; Enter is the only commit key.
function mountTagChipInput(container, { existingTags = [], onAdd, placeholder = 'Add a tag…' } = {}) {
    container.innerHTML = `
        <div class="tag-chip-input-wrap" style="position:relative;">
            <input type="text" class="tag-chip-input" placeholder="${escapeHtml(placeholder)}" />
            <div class="tag-chip-suggestions" style="display:none;position:absolute;top:100%;left:0;right:0;
                 background:var(--card-bg);border:1px solid var(--border-color);z-index:20;max-height:160px;overflow-y:auto;"></div>
        </div>`;
    const input = container.querySelector('.tag-chip-input');
    const suggestBox = container.querySelector('.tag-chip-suggestions');

    function renderSuggestions() {
        const val = input.value.trim().toLowerCase();
        if (!val) { suggestBox.style.display = 'none'; return; }
        const matches = existingTags.filter(t => t.name.toLowerCase().includes(val)).slice(0, 8);
        if (!matches.length) { suggestBox.style.display = 'none'; return; }
        suggestBox.innerHTML = matches.map(t =>
            `<div class="hamburger-menu-item" data-tag="${escapeHtml(t.name)}">${escapeHtml(t.name)} (${t.household_count})</div>`
        ).join('');
        suggestBox.style.display = 'block';
        suggestBox.querySelectorAll('[data-tag]').forEach(el => {
            el.addEventListener('click', () => commit(el.dataset.tag));
        });
    }

    function commit(name) {
        name = name.trim();
        if (!name) return;
        onAdd(name);
        input.value = '';
        suggestBox.style.display = 'none';
    }

    input.addEventListener('input', renderSuggestions);
    input.addEventListener('keydown', (e) => {
        if (e.key === 'Enter') { e.preventDefault(); commit(input.value); }
        if (e.key === 'Escape') { suggestBox.style.display = 'none'; }
    });
    document.addEventListener('click', (e) => {
        if (!container.contains(e.target)) suggestBox.style.display = 'none';
    });
}

function renderTagChips(tags, { onRemove } = {}) {
    return tags.map(t => `
        <span class="tag-chip" data-tag="${escapeHtml(t)}">
            ${escapeHtml(t)}
            ${onRemove ? '<span class="remove" data-remove-tag="' + escapeHtml(t) + '">×</span>' : ''}
        </span>`).join('');
}
