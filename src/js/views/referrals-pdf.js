/**
 * referrals-pdf.js
 * "Record Update Requests" for the ward clerk: a pdfmake PDF to attach
 * to an email, and the same content as plain text to paste into one
 * (docs/Record_Referral_Design.md §3.2). Built client-side like
 * directory-pdf.js; nothing is sent by the app itself.
 *
 * Stage 1 lists the new requests (status to_report) only. The
 * "follow-up" section for overdue referrals needs Stage 2's overdue
 * computation.
 *
 * The contact plan, tags and household comments are never included:
 * only what the clerk needs to update the record.
 */

const ReferralsPdf = {

    _colors() {
        return { heading: '#2c3e50', text: '#000000', detail: '#333333', dim: '#777777' };
    },

    // "MOVED — NEW ADDRESS KNOWN": the kind label, upper case, as the
    // line the clerk scans for.
    _kindLine(r) {
        return referralKindLabel(r.kind).toUpperCase();
    },

    _learnedLine(r) {
        return `Learned ${r.learned_on}.` + (r.details ? ` ${r.details}` : '');
    },

    download(items, unitName) {
        if (!items || items.length === 0) return;
        const colors = this._colors();
        const now = new Date();
        const prepared = localIsoDate(now);

        const content = [
            {
                columns: [
                    { text: 'Record Update Requests', fontSize: 16, bold: true, color: colors.heading },
                    { text: `Prepared ${prepared}`, fontSize: 10, color: colors.dim, alignment: 'right', margin: [0, 5, 0, 0] },
                ],
            },
        ];
        if (unitName) content.push({ text: unitName, fontSize: 11, color: colors.detail, margin: [0, 2, 0, 0] });
        content.push({
            text: `NEW REQUESTS (${items.length})`, fontSize: 11, bold: true, color: colors.heading,
            margin: [0, 16, 0, 8],
        });
        items.forEach(r => content.push(this._entry(r, colors)));

        const docDefinition = {
            pageSize: 'LETTER',
            pageMargins: [54, 54, 54, 50],
            defaultStyle: { font: 'Roboto', fontSize: 10 },
            footer: (currentPage, pageCount) => ({
                margin: [54, 10, 54, 0],
                stack: [
                    { text: `Page ${currentPage} of ${pageCount}`, fontSize: 8, color: colors.dim, alignment: 'center' },
                    { text: 'For Church use only. Information is confidential.', fontSize: 7, italics: true, color: colors.dim, alignment: 'center', margin: [0, 2, 0, 0] },
                ],
            }),
            content,
        };
        const stamp = prepared.replace(/-/g, '');
        pdfMake.createPdf(docDefinition).download(`LostSheep-RecordUpdates-${stamp}.pdf`);
    },

    _entry(r, colors) {
        const right = [
            {
                columns: [
                    { width: '*', text: r.snapshot_name, bold: true, fontSize: 11, color: colors.text },
                    { width: 'auto', text: r.snapshot_address || '(no address on file)', fontSize: 9, color: colors.detail, margin: [8, 1, 0, 0] },
                ],
            },
            { text: this._kindLine(r), bold: true, fontSize: 9, color: colors.heading, margin: [0, 2, 0, 0] },
        ];
        if (r.new_address) right.push({ text: `New address: ${r.new_address}`, fontSize: 10, color: colors.text });
        right.push({ text: this._learnedLine(r), fontSize: 9, color: colors.detail });
        return {
            unbreakable: true,
            margin: [0, 0, 0, 12],
            columns: [
                { width: 48, text: r.reference, bold: true, fontSize: 10, color: colors.heading },
                { width: '*', stack: right },
            ],
        };
    },

    text(items, unitName) {
        const lines = ['Record Update Requests'];
        if (unitName) lines.push(unitName);
        lines.push(`Prepared ${localIsoDate()}`, '', `NEW REQUESTS (${items.length})`, '');
        items.forEach(r => {
            lines.push(`${r.reference}  ${r.snapshot_name} — ${r.snapshot_address || '(no address on file)'}`);
            lines.push(`        ${this._kindLine(r)}`);
            if (r.new_address) lines.push(`        New address: ${r.new_address}`);
            lines.push(`        ${this._learnedLine(r)}`, '');
        });
        return lines.join('\n');
    },
};

window.ReferralsPdf = ReferralsPdf;
