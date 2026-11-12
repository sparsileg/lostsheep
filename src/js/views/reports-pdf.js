/**
 * reports-pdf.js
 * PDF exports for the Reports view (Visit_Reporting_Design.md §5):
 * Needs Attention, Visit Log, Coverage, and the Contact Plans list for
 * ward council (§3.5). Built client-side with the vendored pdfmake, like
 * directory-pdf.js and referrals-pdf.js.
 *
 * The contact plan appears ONLY in contactPlans() (§2.3). The other three
 * never print it, nor the "due" marker that would reveal it.
 */

const ReportsPdf = {

    _colors() {
        return { heading: '#2c3e50', text: '#000000', detail: '#333333', dim: '#777777', rule: '#cccccc', head: '#eef1f4' };
    },

    _stamp() { return localIsoDate().replace(/-/g, ''); },

    // Filename-safe fragment: "Not known" -> "Not-known".
    _slug(s) { return String(s || '').trim().replace(/[^A-Za-z0-9]+/g, '-').replace(/^-|-$/g, '') || 'All'; },

    // Title, optional right-hand note, then sub-lines (unit name, filters).
    _header(title, rightNote, lines) {
        const c = this._colors();
        const out = [{
            columns: [
                { text: title, fontSize: 16, bold: true, color: c.heading },
                { text: rightNote, fontSize: 10, color: c.dim, alignment: 'right', margin: [0, 5, 0, 0] },
            ],
        }];
        lines.filter(Boolean).forEach((l, i) =>
            out.push({ text: l, fontSize: i === 0 ? 11 : 9, color: c.detail, margin: [0, 2, 0, 0] }));
        out.push({ text: '', margin: [0, 0, 0, 10] });
        return out;
    },

    // A full-width table; `widths` per column, `rows` already strings.
    _table(headers, rows, widths, opts = {}) {
        const c = this._colors();
        const body = [
            headers.map(h => ({ text: h, bold: true, fontSize: 9, color: c.heading, fillColor: c.head })),
            ...rows.map(r => r.map(cell => (typeof cell === 'object' && cell !== null) ? cell : { text: cell == null ? '' : String(cell), fontSize: 9 })),
        ];
        return {
            table: { headerRows: 1, widths, body, dontBreakRows: true },
            layout: {
                hLineWidth: (i) => (i <= 1 ? 0.8 : 0.4),
                vLineWidth: () => 0,
                hLineColor: () => c.rule,
                paddingTop: () => opts.padding || 3,
                paddingBottom: () => opts.padding || 3,
            },
            margin: [0, 0, 0, opts.after ?? 10],
        };
    },

    _save(content, filename, footerLabel) {
        const c = this._colors();
        pdfMake.createPdf({
            pageSize: 'LETTER',
            pageMargins: [48, 48, 48, 50],
            defaultStyle: { font: 'Roboto', fontSize: 10 },
            footer: (currentPage, pageCount) => ({
                margin: [48, 10, 48, 0],
                stack: [
                    { text: `${footerLabel} — page ${currentPage} of ${pageCount}`, fontSize: 8, color: c.dim, alignment: 'center' },
                    { text: 'For Church use only. Information is confidential.', fontSize: 7, italics: true, color: c.dim, alignment: 'center', margin: [0, 2, 0, 0] },
                ],
            }),
            content,
        }).download(filename);
    },

    // rows: needs-attention rows with .display; meta: { tagLabel, filterLabel, summary, lastHeader }
    needsAttention(rows, meta) {
        const content = this._header('Needs Attention', `Prepared ${localIsoDate()}`,
            [meta.unitName, `Tag: ${meta.tagLabel} · ${meta.filterLabel}`, meta.summary]);
        content.push(this._table(
            ['Household', 'Address', meta.lastHeader, 'Days', 'Last outcome', 'Attempts'],
            rows.map(r => [
                r.display,
                [r.address_line1, r.city].filter(Boolean).join(', '),
                r.never ? { text: 'Never', bold: true, fontSize: 9 } : (r.last_visit_date || ''),
                r.days_since == null ? '' : String(r.days_since),
                r.last_outcome ? visitKindText({ method: r.last_method, outcome: r.last_outcome }) : '',
                r.attempts ? String(r.attempts) : '',
            ]),
            ['22%', '*', 62, 30, 90, 42],
        ));
        this._save(content, `LostSheep-Needs-Attention-${this._slug(meta.tagLabel)}-${this._stamp()}.pdf`, 'Needs Attention');
    },

    // rows: visit-log rows in screen order; meta: { from, to, tagLabel, summary }
    visitLog(rows, meta) {
        const content = this._header('Visit Log', `Prepared ${localIsoDate()}`,
            [meta.unitName, `${meta.from} to ${meta.to} · Tag: ${meta.tagLabel}`, meta.summary]);
        content.push(this._table(
            ['Household', 'Date', 'How', 'Comments'],
            rows.map(r => [r.household_name, r.visit_date, visitKindText(r), r.comments || '']),
            ['24%', 62, 110, '*'],
        ));
        this._save(content, `LostSheep-Visit-Log-${meta.from}-${meta.to}.pdf`, 'Visit Log');
    },

    // result: get_coverage; meta: { from, to, periodLabel, reachedWord }
    coverage(result, meta) {
        const pct = r => r.households ? `${Math.round(100 * r.reached_in_period / r.households)}%` : '—';
        const row = (r, bold) => [r.tag || '(No tag)', r.households, r.never, r.reached_in_period, pct(r)]
            .map(v => ({ text: String(v), fontSize: 9, bold: !!bold }));
        const content = this._header('Coverage', `Prepared ${localIsoDate()}`,
            [meta.unitName, `${meta.periodLabel}: ${meta.from} to ${meta.to}`,
                `Coverage = households ${meta.reachedWord} in the period. Do-not-contact households are not counted.`]);
        content.push(this._table(
            ['Tag', 'Households', `Never ${meta.reachedWord}`, `${meta.reachedWord[0].toUpperCase()}${meta.reachedWord.slice(1)} in period`, 'Coverage'],
            [...result.rows.map(r => row(r)), row({ ...result.total, tag: 'Total' }, true)],
            ['*', 70, 90, 100, 60],
        ));
        this._save(content, `LostSheep-Coverage-${this._stamp()}.pdf`, 'Coverage');
    },

    // The ward council list (§3.5). Name, plan, last contact date and a
    // blank Adjustment column only: no addresses, no comments, no notes.
    // groups: [{ label, rows }] for Monthly / Quarterly / As needed;
    // dnc: rows of do-not-contact households. Rows carry .display.
    contactPlans(groups, dnc, unitName) {
        const c = this._colors();
        const content = [{
            columns: [
                { text: 'Contact Plans', fontSize: 16, bold: true, color: c.heading },
                { text: 'Confidential — ward council', fontSize: 10, bold: true, color: c.heading, alignment: 'right', margin: [0, 5, 0, 0] },
            ],
        }, {
            columns: [
                { text: unitName || '', fontSize: 11, color: c.detail },
                { text: `Prepared ${localIsoDate()}`, fontSize: 10, color: c.dim, alignment: 'right' },
            ],
            margin: [0, 2, 0, 12],
        }];
        const blank = { text: '', fontSize: 9 };
        groups.forEach(g => {
            content.push({ text: `${g.label.toUpperCase()} (${g.rows.length})`, fontSize: 11, bold: true, color: c.heading, margin: [0, 6, 0, 4] });
            if (!g.rows.length) {
                content.push({ text: 'None.', fontSize: 9, color: c.dim, margin: [0, 0, 0, 8] });
                return;
            }
            content.push(this._table(
                ['Household', 'Last contact', 'Adjustment'],
                g.rows.map(r => [r.display, r.last_contact_date || 'None yet', blank]),
                ['*', 80, 170],
                { padding: 6 },
            ));
        });
        if (dnc.length) {
            content.push({ text: `DO NOT CONTACT — at the member's request (${dnc.length})`, fontSize: 11, bold: true, color: c.heading, margin: [0, 6, 0, 4] });
            content.push(this._table(['Household'], dnc.map(r => [r.display]), ['*']));
        }
        this._save(content, `LostSheep-Contact-Plans-${this._stamp()}.pdf`, 'Contact Plans');
    },
};

window.ReportsPdf = ReportsPdf;
