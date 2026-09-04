/**
 * directory-pdf.js
 * PDF directory export for a tagged (or search-filtered) group of
 * households from the Households view. Issue #15.
 *
 * Deliberately a separate file from households-view.js, matching the
 * session-report-pdf.js / session-report-view.js split precedent from
 * Astryx: different rendering target (pdfmake document-definition
 * objects, not DOM), different failure mode (does the PDF match the
 * source directory's layout, not does the on-screen table compute the
 * right rows).
 *
 * pdfmake runs entirely client-side (build a docDefinition, download a
 * blob) — no OS print dialog, no dependency on WebKitGTK's print
 * pipeline, no new Rust crate. Loaded as a plain global (pdfMake), same
 * as this app already loads pako for gzip in backup-restore.js.
 *
 * Layout per household (two inner columns, matching the source
 * directory's own layout — see issue #15 screencap):
 *   Left  — household surname line, then head #1's name/phone/email,
 *           then head #2's block (if present) stacked directly under it.
 *   Right — address block only (street, city/state/zip, lat/lon).
 * Each entry is wrapped `unbreakable: true` so a single household never
 * splits across a page boundary (issue #15 acceptance criteria).
 *
 * Every page carries a "Lost Sheep - <tag>" header (<tag> is the active
 * tag filter's name, or "All" when none is set — "All" isn't a real tag,
 * just every household regardless of tag) and a "Page x of y" footer.
 */

const DirectoryPdf = {

    _colors() {
        return {
            headingText: '#2c3e50',
            nameText: '#000000',
            detailText: '#333333',
            latLonText: '#777777',
        };
    },

    // -------------------------------------------------------------------------
    // Entry point
    // -------------------------------------------------------------------------

    download(households, tagLabel) {
        if (!households || households.length === 0) return;
        const colors = this._colors();
        const label = tagLabel || 'All';
        const now = new Date();

        const content = households.map(h => this._entry(h, colors));

        const docDefinition = {
            pageSize: 'LETTER',
            pageMargins: [54, 70, 54, 40],
            defaultStyle: { font: 'Roboto', fontSize: 10 },
            header: {
                text: `Lost Sheep - ${label}`,
                fontSize: 9,
                bold: true,
                color: colors.headingText,
                alignment: 'center',
                margin: [0, 20, 0, 0],
            },
            footer: (currentPage, pageCount) => {
                const date = this._formattedDate(now);
                const isOdd = currentPage % 2 === 1;
                return {
                    margin: [54, 10, 54, 0],
                    columns: [
                        { width: 150, text: isOdd ? '' : date, fontSize: 8, color: colors.latLonText, alignment: 'left' },
                        { width: '*', text: `Page ${currentPage} of ${pageCount}`, fontSize: 8, color: colors.latLonText, alignment: 'center' },
                        { width: 150, text: isOdd ? date : '', fontSize: 8, color: colors.latLonText, alignment: 'right' },
                    ],
                };
            },
            content,
        };

        const filename = `LostSheep-${label.replace(/\s+/g, '_')}-${this._timestamp(now)}.pdf`;
        pdfMake.createPdf(docDefinition).download(filename);
    },

    _monthAbbrev() {
        return ['Jan', 'Feb', 'Mar', 'Apr', 'May', 'Jun', 'Jul', 'Aug', 'Sep', 'Oct', 'Nov', 'Dec'];
    },

    // MMM DD, YYYY — same calendar date as the filename's YYYYMMDD, just
    // formatted for print rather than for a filesystem-safe filename.
    // Not locale-dependent (toLocaleDateString would vary by OS locale);
    // this always renders the same way regardless of machine settings.
    _formattedDate(d) {
        const month = this._monthAbbrev()[d.getMonth()];
        const day = String(d.getDate()).padStart(2, '0');
        return `${month} ${day}, ${d.getFullYear()}`;
    },

    _timestamp(d) {
        const pad = (n) => String(n).padStart(2, '0');
        return `${d.getFullYear()}${pad(d.getMonth() + 1)}${pad(d.getDate())}`;
    },

    // -------------------------------------------------------------------------
    // One household entry — two columns, kept together on one page.
    // -------------------------------------------------------------------------

    _entry(h, colors) {
        return {
            unbreakable: true,
            margin: [0, 0, 0, 14],
            columns: [
                { width: '55%', stack: this._leftColumn(h, colors) },
                { width: '45%', stack: this._rightColumn(h, colors) },
            ],
        };
    },

    _leftColumn(h, colors) {
        const stack = [
            { text: formatDirectoryName(h), fontSize: 13, bold: true, color: colors.headingText, margin: [0, 0, 0, 6] },
        ];
        stack.push(...this._headBlock(h.first_name, h.phone_1, h.email_1, colors));
        if (h.first_name_2) {
            const name2 = h.last_name_2 ? `${h.first_name_2} ${h.last_name_2}` : h.first_name_2;
            stack.push({ text: '', margin: [0, 6, 0, 0] });
            stack.push(...this._headBlock(name2, h.phone_2, h.email_2, colors));
        }
        return stack;
    },

    _rightColumn(h, colors) {
        const stack = [];

        const addressLines = [h.address_line1, h.address_line2].filter(Boolean);
        addressLines.forEach(l => stack.push({ text: l, fontSize: 10, color: colors.detailText }));
        const cityLine = [h.city, h.state].filter(Boolean).join(' ') + (h.zip ? ' ' + h.zip : '');
        if (cityLine.trim()) stack.push({ text: cityLine.trim(), fontSize: 10, color: colors.detailText });
        if (h.latitude != null && h.longitude != null) {
            stack.push({ text: `${h.latitude}, ${h.longitude}`, fontSize: 8, color: colors.latLonText, margin: [0, 2, 0, 0] });
        }

        return stack;
    },

    // One head's name/phone/email — indented, matching the screencap's
    // sub-name treatment under a household's surname line. Both heads
    // use the same indent and font size now that they're stacked
    // together in the left column (previously the second head used a
    // different, larger font size by mistake — fixed here).
    _headBlock(name, phone, email, colors) {
        const block = [
            { text: name, fontSize: 11, margin: [14, 0, 0, 1], color: colors.nameText },
        ];
        if (phone) block.push({ text: phone, fontSize: 9, margin: [14, 0, 0, 0], color: colors.detailText });
        if (email) block.push({ text: email, fontSize: 9, margin: [14, 0, 0, 0], color: colors.detailText });
        return block;
    },

};

window.DirectoryPdf = DirectoryPdf;
