// ── Visit method / outcome / contact plan vocabulary ────────────────────
// Visit_Reporting_Design.md §2.1 and §2.3. The values match the CHECK
// lists in schema.sql and the validation in commands/visits.rs
// (VisitKind) and commands/households.rs (set_contact_plan_query).
// Shared by the household page, Needs Attention and the Visit Log.

const VISIT_METHODS = [
    { value: 'visit', label: 'Visit' },
    { value: 'phone', label: 'Phone' },
    { value: 'card', label: 'Card' },
];

// 'sent' is not offered: it is the only outcome a card has, and the
// outcome picker is hidden for cards.
const VISIT_OUTCOMES = [
    { value: 'contacted', label: 'Contacted' },
    { value: 'not_home', label: 'Not home' },
    { value: 'no_answer', label: 'No answer' },
    { value: 'refused', label: 'Declined' },
    { value: 'moved', label: 'Moved' },
    { value: 'deceased', label: 'Deceased' },
    { value: 'other', label: 'Other' },
];

const CONTACT_PLANS = [
    { value: '', label: 'None' },
    { value: 'monthly', label: 'Monthly' },
    { value: 'quarterly', label: 'Quarterly' },
    { value: 'as_needed', label: 'As needed' },
];

function visitMethodLabel(m) {
    return (VISIT_METHODS.find(x => x.value === m) || { label: m || '' }).label;
}

function visitOutcomeLabel(o) {
    if (o === 'sent') return 'Sent';
    return (VISIT_OUTCOMES.find(x => x.value === o) || { label: o || '' }).label;
}

function contactPlanLabel(p) {
    return p ? (CONTACT_PLANS.find(x => x.value === p) || { label: p }).label : '';
}

// The settled rule (§2.1): only a contacted visit or call is contact.
function visitIsContact(v) {
    return v.outcome === 'contacted' && (v.method === 'visit' || v.method === 'phone');
}

// "Visit · Contacted", "Card", "Phone · No answer", with "· Invited".
// Plain text; callers escape it.
function visitKindText(v) {
    const parts = [visitMethodLabel(v.method || 'visit')];
    if (v.method !== 'card') parts.push(visitOutcomeLabel(v.outcome || 'contacted'));
    if (v.invited) parts.push('Invited');
    return parts.join(' · ');
}

// The method / outcome / invitation controls, used by "Record New Visit"
// and the inline visit editor. `el` gets the markup; returns { get() }
// giving { method, outcome, invited }. Themed dropdowns, not native
// selects (WebKitGTK draws those unreadably in the dark themes).
function mountVisitKindControls(el, initial = {}) {
    const state = {
        method: initial.method || 'visit',
        outcome: initial.outcome && initial.outcome !== 'sent' ? initial.outcome : 'contacted',
        invited: !!initial.invited,
    };
    el.innerHTML = `
        <div class="vk-row">
            <label class="vk-field">Method <div class="vk-dropdown" data-vk="method"></div></label>
            <label class="vk-field" data-vk-outcome-wrap>Outcome <div class="vk-dropdown" data-vk="outcome"></div></label>
        </div>
        <label class="vk-invited"><input type="checkbox" data-vk="invited" ${state.invited ? 'checked' : ''}> Invitation extended</label>`;
    const outcomeWrap = el.querySelector('[data-vk-outcome-wrap]');
    const sync = () => { outcomeWrap.style.display = state.method === 'card' ? 'none' : ''; };
    mountDropdown(el.querySelector('[data-vk="method"]'), {
        items: VISIT_METHODS, value: state.method, sizeToItems: true,
        onSelect: (v) => { state.method = v; sync(); },
    });
    mountDropdown(el.querySelector('[data-vk="outcome"]'), {
        items: VISIT_OUTCOMES, value: state.outcome, sizeToItems: true,
        onSelect: (v) => { state.outcome = v; },
    });
    el.querySelector('[data-vk="invited"]').addEventListener('change', (e) => { state.invited = e.target.checked; });
    sync();
    return {
        get: () => ({
            method: state.method,
            outcome: state.method === 'card' ? 'sent' : state.outcome,
            invited: state.invited,
        }),
    };
}

window.VISIT_METHODS = VISIT_METHODS;
window.VISIT_OUTCOMES = VISIT_OUTCOMES;
window.CONTACT_PLANS = CONTACT_PLANS;
