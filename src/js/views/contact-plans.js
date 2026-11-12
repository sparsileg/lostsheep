// ── Contact Plans (Visit_Reporting_Design.md §3.5) ────────────────────────
// Who are we committed to reaching, and how often? Every household with a
// plan, grouped Monthly / Quarterly / As needed (by last name within each),
// with the plan changeable inline, then the do-not-contact households.
// Export PDF is the list the user takes to ward council: names, plan and
// last contact date only (ReportsPdf.contactPlans).
//
// A tab of the Reports view (visits-report-view.js), which calls
// ContactPlans.mount() once and refresh() whenever the tab is shown.

const cpState = { rows: [] };

const CP_GROUPS = CONTACT_PLANS.filter(p => p.value);

// Do-not-contact households are listed only under Do not contact, even
// if they also carry a plan: the member's request outranks our plan.
function cpGrouped() {
    const rows = cpState.rows.map(r => ({ ...r, display: naFormatName(r) }));
    return {
        groups: CP_GROUPS.map(g => ({
            value: g.value,
            label: g.label,
            rows: rows.filter(r => !r.do_not_contact && r.contact_plan === g.value),
        })),
        dnc: rows.filter(r => r.do_not_contact),
    };
}

const ContactPlans = {
    mount(container) {
        container.innerHTML = `
            <div class="na-toolbar">
                <button class="btn" id="cpPdfBtn">Export PDF for ward council</button>
            </div>
            <div class="na-summary" id="cpSummary"></div>
            <div id="cpGroups"></div>
        `;
        document.getElementById('cpPdfBtn').addEventListener('click', () => ContactPlans.exportPdf());
    },

    async refresh() {
        try {
            cpState.rows = await Api.getContactPlans();
        } catch (e) {
            showMessage(`${e}`, CONSTANTS.MESSAGE_TYPES.ERROR);
            return;
        }
        renderContactPlans();
    },

    async exportPdf() {
        const { groups, dnc } = cpGrouped();
        if (!groups.some(g => g.rows.length) && !dnc.length) {
            showMessage('No household has a contact plan yet.', CONSTANTS.MESSAGE_TYPES.INFO);
            return;
        }
        let unitName = '';
        try { unitName = (await Api.getActiveProfile())?.name || ''; } catch (_) { /* optional header line */ }
        ReportsPdf.contactPlans(groups, dnc, unitName);
    },
};

function renderContactPlans() {
    const { groups, dnc } = cpGrouped();
    const planned = groups.reduce((n, g) => n + g.rows.length, 0);
    const due = groups.reduce((n, g) => n + g.rows.filter(r => r.plan_due).length, 0);
    document.getElementById('cpSummary').textContent = planned
        ? `${planned} household${planned === 1 ? '' : 's'} with a contact plan` +
          (due ? ` — ${due} due.` : '.') +
          ' Set a plan from a household’s page, or change one here.'
        : 'No household has a contact plan yet. Plans are set from a household’s page, or offered after a first contact with a Not known household.';

    const address = r => escapeHtml([r.address_line1, r.city].filter(Boolean).join(', '));
    const section = (title, body) => `<h3 class="cp-heading">${escapeHtml(title)}</h3>${body}`;
    let html = groups.map(g => section(`${g.label} (${g.rows.length})`, g.rows.length ? `
        <table class="cp-table">
            <thead><tr><th>Household</th><th>Address</th><th>Plan</th><th>Last contact</th></tr></thead>
            <tbody>${g.rows.map(r => `
                <tr>
                    <td><a href="#" class="cp-open" data-cp-open="${r.household_id}">${escapeHtml(r.display)}</a></td>
                    <td>${address(r)}</td>
                    <td><div class="cp-plan" data-cp-plan="${r.household_id}" data-cp-value="${escapeHtml(r.contact_plan || '')}"></div></td>
                    <td>${r.last_contact_date ? escapeHtml(r.last_contact_date) : '<span class="na-never">Never</span>'}${r.plan_due ? ' <span class="na-due">due</span>' : ''}</td>
                </tr>`).join('')}
            </tbody>
        </table>` : '<p class="cp-none">None.</p>')).join('');
    if (dnc.length) {
        html += section(`Do not contact — at the member’s request (${dnc.length})`, `
            <table class="cp-table">
                <thead><tr><th>Household</th><th>Address</th></tr></thead>
                <tbody>${dnc.map(r => `
                    <tr>
                        <td><a href="#" class="cp-open" data-cp-open="${r.household_id}">${escapeHtml(r.display)}</a></td>
                        <td>${address(r)}</td>
                    </tr>`).join('')}
                </tbody>
            </table>`);
    }
    const root = document.getElementById('cpGroups');
    root.innerHTML = html;

    root.querySelectorAll('[data-cp-open]').forEach(a => a.addEventListener('click', (e) => {
        e.preventDefault();
        openHousehold(Number(a.dataset.cpOpen));
    }));
    root.querySelectorAll('[data-cp-plan]').forEach(el => {
        const id = Number(el.dataset.cpPlan);
        mountDropdown(el, {
            items: CONTACT_PLANS, value: el.dataset.cpValue, sizeToItems: true,
            onSelect: async (plan) => {
                try {
                    await Api.setContactPlan(id, plan || null);
                } catch (e) {
                    showMessage(`${e}`, CONSTANTS.MESSAGE_TYPES.ERROR);
                }
                await ContactPlans.refresh();
            },
        });
    });
}

window.ContactPlans = ContactPlans;
