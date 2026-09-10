/**
 * potential-problems-pdf.js
 * PDF export for the Potential Problems diagnostic list (issue #48
 * follow-up) — a separate, offline-workable checklist so households can
 * be examined one at a time without the app open.
 *
 * Same pdfmake pattern as directory-pdf.js: client-side, build a
 * docDefinition, download the blob — no OS print dialog dependency.
 * Deliberately its own file, matching that split's precedent (different
 * rendering target from the on-screen modal, not a shared concern).
 */

const PotentialProblemsPdf = {

    _colors() {
        return {
            headingText: '#2c3e50',
            nameText: '#000000',
            detailText: '#333333',
            reasonText: '#8a1f1f',
        };
    },

    download(problems) {
        if (!problems || problems.length === 0) return;
        const colors = this._colors();
        const now = new Date();

        // Households with no address on file get no address/road/geocoord
        // findings at all — diagnostics.rs only ever pushes the single
        // "No address on file" reason for them (see find_potential_problems)
        // — so they carry nothing worth a full entry. Pulled out to their
        // own name-only list at the end of the report instead.
        const noAddress = problems.filter(p => p.reasons.length === 1 && p.reasons[0] === 'No address on file');
        const withFindings = problems.filter(p => !(p.reasons.length === 1 && p.reasons[0] === 'No address on file'));

        // Grouped and sorted by tag, in a fixed priority order (Stan's
        // call — not-yet-contacted households surface first). Matched
        // case-insensitively since the stored tag text is "Not known"
        // (lowercase k) per households-view.js's targetTag values, but
        // headings print in the canonical casing below regardless of how
        // the tag happens to be cased in the database. Anything untagged,
        // or tagged with something outside these three, lands in its own
        // trailing group rather than being silently dropped.
        const TAG_ORDER = ['Not Known', 'Known', 'Do Not Contact'];
        const byTag = new Map(TAG_ORDER.map(t => [t.toLowerCase(), []]));
        const other = [];
        withFindings.forEach(p => {
            const bucket = byTag.get((p.tag || '').toLowerCase());
            if (bucket) bucket.push(p); else other.push(p);
        });

        // Each of the four possible sections (three tags + no-address)
        // starts on its own page, except whichever one happens to be
        // first — pdfmake's pageBreak: 'before' on the very first content
        // node would otherwise print a blank leading page. "Untagged" is
        // included in the rotation for consistency even though Stan only
        // asked about the four named sections; it's an edge case that
        // shouldn't normally have anything in it.
        const sections = [];
        TAG_ORDER.forEach(tagLabel => {
            const items = byTag.get(tagLabel.toLowerCase());
            if (items.length > 0) sections.push({ label: tagLabel, items });
        });
        if (other.length > 0) sections.push({ label: 'Untagged', items: other });
        if (noAddress.length > 0) sections.push({ label: 'No address on file', items: noAddress, nameOnly: true });

        const content = [];
        sections.forEach((section, i) => {
            content.push({
                text: section.label,
                fontSize: 12,
                bold: true,
                color: colors.headingText,
                margin: [0, 12, 0, 6],
                ...(i > 0 ? { pageBreak: 'before' } : {}),
            });
            section.items.forEach(p => {
                content.push(section.nameOnly
                    ? { text: p.household_name || '(no name on file)', fontSize: 10, color: colors.detailText, margin: [0, 0, 0, 2] }
                    : this._entry(p, colors));
            });
        });

        const docDefinition = {
            pageSize: 'LETTER',
            pageMargins: [54, 70, 54, 40],
            defaultStyle: { font: 'Roboto', fontSize: 10 },
            header: {
                text: 'Lost Sheep - Data Validation Findings',
                fontSize: 9,
                bold: true,
                color: colors.headingText,
                alignment: 'center',
                margin: [0, 20, 0, 0],
            },
            footer: (currentPage, pageCount) => ({
                margin: [54, 10, 54, 0],
                columns: [
                    { width: 150, text: this._formattedDate(now), fontSize: 8, color: colors.detailText, alignment: 'left' },
                    { width: '*', text: `Page ${currentPage} of ${pageCount}`, fontSize: 8, color: colors.detailText, alignment: 'center' },
                    { width: 150, text: '' },
                ],
            }),
            content,
        };

        pdfMake.createPdf(docDefinition).download(`LostSheep-PotentialProblems-${this._timestamp(now)}.pdf`);
    },

    // One household entry — name/address, then a bulleted reason per
    // trigger. Kept together on one page, same as directory-pdf.js's
    // per-household entries.
    _entry(p, colors) {
        const nameLine = p.tag
            ? `${p.household_name || '(no name on file)'} (${p.tag})`
            : (p.household_name || '(no name on file)');
        const stack = [
            { text: nameLine, fontSize: 12, bold: true, color: colors.headingText },
            { text: p.address_line1 || '(no address on file)', fontSize: 10, color: colors.detailText, margin: [0, 1, 0, 4] },
        ];
        p.reasons.forEach(r => {
            stack.push({ text: `\u2022 ${r}`, fontSize: 9, color: colors.reasonText, margin: [10, 0, 0, 1] });
        });
        return { unbreakable: true, margin: [0, 0, 0, 12], stack };
    },

    _monthAbbrev() {
        return ['Jan', 'Feb', 'Mar', 'Apr', 'May', 'Jun', 'Jul', 'Aug', 'Sep', 'Oct', 'Nov', 'Dec'];
    },

    _formattedDate(d) {
        const month = this._monthAbbrev()[d.getMonth()];
        const day = String(d.getDate()).padStart(2, '0');
        return `${month} ${day}, ${d.getFullYear()}`;
    },

    _timestamp(d) {
        const pad = (n) => String(n).padStart(2, '0');
        return `${d.getFullYear()}${pad(d.getMonth() + 1)}${pad(d.getDate())}`;
    },

};

window.PotentialProblemsPdf = PotentialProblemsPdf;
