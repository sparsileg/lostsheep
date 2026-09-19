/**
 * data-validation-pdf.js
 * PDF export for the Potential Problems diagnostic list (issue #48
 * follow-up) — a separate, offline-workable checklist so households can
 * be examined one at a time without the app open.
 *
 * Same pdfmake pattern as directory-pdf.js: client-side, build a
 * docDefinition, download the blob — no OS print dialog dependency.
 * Deliberately its own file, matching that split's precedent (different
 * rendering target from the on-screen modal, not a shared concern).
 */

// Priority order for the "No address on file" section specifically — Stan's
// call, and deliberately NOT the same order as the main report's TAG_ORDER
// (below, in download()): a household with no address on file *and* tagged
// Do Not Contact may have no other way to be reached at all, so that group
// surfaces first here even though Not Known leads everywhere else in the
// report. "Not Known" is the canonical label used throughout this report
// (matches households-view.js's tag casing); Stan's shorthand for it in
// conversation was "Unknown."
const NO_ADDRESS_TAG_ORDER = ['Do Not Contact', 'Not Known', 'Known'];

const PotentialProblemsPdf = {

    _colors() {
        return {
            headingText: '#2c3e50',
            nameText: '#000000',
            detailText: '#333333',
            reasonText: '#8a1f1f',
        };
    },

    download(problems, roadsChecked = true) {
        // Issue #80: previously guarded on problems alone — an empty/
        // un-ingested roads.db with zero flagged households produced no
        // PDF at all, same silent-degradation gap the on-screen modal
        // had. Still skip when there's genuinely nothing to say: real
        // checks ran (roadsChecked) and found nothing (empty problems).
        if ((!problems || problems.length === 0) && roadsChecked) return;
        const colors = this._colors();
        const now = new Date();

        // Shared-address group entries (household_ids.length > 1) carry
        // multiple people and no single tag — handled as their own
        // section below, not folded into the tag-based bucketing (a
        // group can span residents with different tags, so there's no
        // single tag section it belongs in).
        const groups = problems.filter(p => p.household_ids && p.household_ids.length > 1);
        const singles = problems.filter(p => !(p.household_ids && p.household_ids.length > 1));

        // Households with no address on file get no address/road/geocoord
        // findings at all — diagnostics.rs only ever pushes the single
        // "No address on file" reason for them (see find_potential_problems)
        // — so they carry nothing worth a full entry. Pulled out to their
        // own name-only list at the end of the report instead.
        const noAddress = singles.filter(p => p.reasons.length === 1 && p.reasons[0] === 'No address on file');
        const withFindings = singles.filter(p => !(p.reasons.length === 1 && p.reasons[0] === 'No address on file'));

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
        if (groups.length > 0) sections.push({ label: 'Shared Address — Geocoordinate Mismatch', items: groups, isGroup: true });
        // Sorted/grouped by tag within their own section (Stan's call —
        // Do Not Contact first here, a different priority than the main
        // TAG_ORDER above: someone with no address on file is most urgent
        // to track down when they're also flagged Do Not Contact, since
        // there may be no other way to reach them at all).
        if (noAddress.length > 0) {
            sections.push({
                label: 'No address on file',
                nameOnly: true,
                tagGroups: this._groupByTag(noAddress, NO_ADDRESS_TAG_ORDER),
            });
        }

        const content = [];

        // Issue #80: printed before any section, in the same reason/
        // warning color used for flagged findings, so it can't be missed
        // even if the person only skims page 1 — this is the "clearly
        // marked on the PDF" signal for a roads.db that had no data to
        // check against, distinct from "checks ran, found nothing."
        if (!roadsChecked) {
            content.push({
                text: 'Road database has no data for this scan — street-name checks were skipped for every household. Re-ingest under Road Management to restore them.',
                fontSize: 10,
                bold: true,
                color: colors.reasonText,
                margin: [0, 0, 0, 16],
            });
        }

        sections.forEach((section, i) => {
            content.push({
                text: section.label,
                fontSize: 12,
                bold: true,
                color: colors.headingText,
                margin: [0, 12, 0, 6],
                ...(i > 0 ? { pageBreak: 'before' } : {}),
            });
            if (section.tagGroups) {
                // No-address section: sub-headed by tag (see
                // NO_ADDRESS_TAG_ORDER) instead of one flat name list.
                section.tagGroups.forEach((tg, j) => {
                    content.push({
                        text: tg.label,
                        fontSize: 10,
                        bold: true,
                        italics: true,
                        color: colors.headingText,
                        margin: [0, j > 0 ? 8 : 0, 0, 3],
                    });
                    tg.items.forEach(p => {
                        content.push({ text: p.household_name || '(no name on file)', fontSize: 10, color: colors.detailText, margin: [10, 0, 0, 2] });
                    });
                });
            } else {
                section.items.forEach(p => {
                    content.push(section.nameOnly
                        ? { text: this._nameWithTag(p.household_name, p.tag), fontSize: 10, color: colors.detailText, margin: [0, 0, 0, 2] }
                        : section.isGroup
                            ? this._groupEntry(p, colors)
                            : this._entry(p, colors));
                });
            }
        });

        const docDefinition = {
            pageSize: 'LETTER',
            pageMargins: [54, 70, 54, 40],
            defaultStyle: { font: 'Roboto', fontSize: 10 },
            header: {
                margin: [0, 20, 0, 0],
                stack: [
                    { text: 'Lost Sheep - Data Validation Findings', fontSize: 9, bold: true, color: colors.headingText, alignment: 'center' },
                    { text: 'For Church use only. Information is confidential.', fontSize: 7, italics: true, color: colors.headingText, alignment: 'center', margin: [0, 2, 0, 0] },
                ],
            },
            footer: (currentPage, pageCount) => ({
                margin: [54, 10, 54, 0],
                stack: [
                    {
                        columns: [
                            { width: 150, text: this._formattedDate(now), fontSize: 8, color: colors.detailText, alignment: 'left' },
                            { width: '*', text: `Page ${currentPage} of ${pageCount}`, fontSize: 8, color: colors.detailText, alignment: 'center' },
                            { width: 150, text: '' },
                        ],
                    },
                    { text: 'For Church use only. Information is confidential.', fontSize: 7, italics: true, color: colors.detailText, alignment: 'center', margin: [0, 2, 0, 0] },
                ],
            }),
            content,
        };

        pdfMake.createPdf(docDefinition).download(`LostSheep-DataValidation-${this._timestamp(now)}.pdf`);
    },

    // One household entry — name/address, then a bulleted reason per
    // trigger. Kept together on one page, same as directory-pdf.js's
    // per-household entries.
    _entry(p, colors) {
        const nameLine = this._nameWithTag(p.household_name, p.tag);
        const addressLine = p.address_line1
            ? (p.address_line2 ? `${p.address_line1}, ${p.address_line2}` : p.address_line1)
            : '(no address on file)';
        const stack = [
            { text: nameLine, fontSize: 12, bold: true, color: colors.headingText },
            { text: addressLine, fontSize: 10, color: colors.detailText, margin: [0, 1, 0, 4] },
        ];
        p.reasons.forEach(r => {
            stack.push({ text: `\u2022 ${r}`, fontSize: 9, color: colors.reasonText, margin: [10, 0, 0, 1] });
        });
        return { unbreakable: true, margin: [0, 0, 0, 12], stack };
    },

    // One shared-address group — address line, every resident's name
    // listed (this is the whole point: seeing everyone at the address
    // together is what makes the source problem fixable), then the
    // reason(s) same as a normal entry.
    _groupEntry(p, colors) {
        const addressLine = p.address_line1
            ? (p.address_line2 ? `${p.address_line1}, ${p.address_line2}` : p.address_line1)
            : '(no address on file)';
        const stack = [
            { text: addressLine, fontSize: 12, bold: true, color: colors.headingText, margin: [0, 0, 0, 4] },
        ];
        // household_tags is parallel to household_names (same index = same
        // household) — each resident's own tag shown next to their name,
        // since the group as a whole has no single tag (members may differ).
        const names = p.household_names || [p.household_name];
        const tags = p.household_tags || [];
        names.forEach((name, i) => {
            stack.push({ text: this._nameWithTag(name, tags[i]), fontSize: 10, color: colors.detailText, margin: [10, 0, 0, 1] });
        });
        p.reasons.forEach(r => {
            stack.push({ text: `\u2022 ${r}`, fontSize: 9, color: colors.reasonText, margin: [10, 4, 0, 1] });
        });
        return { unbreakable: true, margin: [0, 0, 0, 12], stack };
    },

    // Shared name+tag formatting — used by _entry and _groupEntry (per
    // member). No longer used by the no-address-on-file list (ad hoc
    // request — sort/group that list by tag): that list is now sub-headed
    // by tag (see _groupByTag), so repeating
    // the tag on every name underneath its own heading would be redundant.
    _nameWithTag(name, tag) {
        const n = name || '(no name on file)';
        return tag ? `${n} (${tag})` : n;
    },

    // Ad hoc request — sort/group the no-address list by tag. Buckets
    // `items` by their `tag` field into the given
    // priority order (case-insensitive match against the stored tag text,
    // same reasoning as download()'s byTag grouping above), sorting each
    // bucket's households alphabetically by name for a scannable list.
    // Anything untagged, or tagged with something outside `tagOrder`,
    // lands in a trailing "Untagged" bucket rather than being dropped.
    // Returns an array of { label, items }, omitting empty buckets.
    _groupByTag(items, tagOrder) {
        const byTag = new Map(tagOrder.map(t => [t.toLowerCase(), []]));
        const other = [];
        items.forEach(p => {
            const bucket = byTag.get((p.tag || '').toLowerCase());
            if (bucket) bucket.push(p); else other.push(p);
        });
        const sortByName = (a, b) => (a.household_name || '').localeCompare(b.household_name || '');
        const out = [];
        tagOrder.forEach(label => {
            const bucket = byTag.get(label.toLowerCase());
            if (bucket.length > 0) out.push({ label, items: [...bucket].sort(sortByName) });
        });
        if (other.length > 0) out.push({ label: 'Untagged', items: [...other].sort(sortByName) });
        return out;
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
