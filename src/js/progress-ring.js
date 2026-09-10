// progress-ring.js — small floating circular progress indicator, shown
// during long-running backend scans (Data Validation / find_potential_
// problems). Purely a DOM/visual widget — callers own starting/stopping
// it and feeding it percentages. Deliberately a plain script, not a
// module: sidebar.js (a classic script) and any future module caller can
// both reach it via window.ProgressRing with no import wiring needed.
const ProgressRing = (() => {
    const SIZE = 44;
    const STROKE = 4;
    const RADIUS = (SIZE - STROKE) / 2;
    const CIRCUMFERENCE = 2 * Math.PI * RADIUS;

    let el = null;
    let baseLabel = '';

    function build() {
        const wrap = document.createElement('div');
        wrap.className = 'progress-ring-overlay';
        wrap.innerHTML = `
            <div class="progress-ring">
                <svg width="${SIZE}" height="${SIZE}" viewBox="0 0 ${SIZE} ${SIZE}">
                    <circle class="progress-ring-track" cx="${SIZE / 2}" cy="${SIZE / 2}" r="${RADIUS}" stroke-width="${STROKE}"></circle>
                    <circle class="progress-ring-fill" cx="${SIZE / 2}" cy="${SIZE / 2}" r="${RADIUS}" stroke-width="${STROKE}"
                        stroke-dasharray="${CIRCUMFERENCE}" stroke-dashoffset="${CIRCUMFERENCE}"
                        transform="rotate(-90 ${SIZE / 2} ${SIZE / 2})"></circle>
                </svg>
            </div>
            <span class="progress-ring-label"></span>
        `;
        document.body.appendChild(wrap);
        return wrap;
    }

    return {
        // label: plain text shown next to the ring, followed by a live
        // percentage once update() starts being called.
        show(label) {
            this.hide();
            baseLabel = label || '';
            el = build();
            el.querySelector('.progress-ring-label').textContent = baseLabel;
        },
        // pct: 0-100. Clockwise fill via stroke-dashoffset — full
        // circumference (fully hidden) at 0%, zero offset (fully drawn)
        // at 100%.
        update(pct) {
            if (!el) return;
            const clamped = Math.max(0, Math.min(100, pct));
            const offset = CIRCUMFERENCE * (1 - clamped / 100);
            el.querySelector('.progress-ring-fill').style.strokeDashoffset = offset;
            el.querySelector('.progress-ring-label').textContent = `${baseLabel} ${Math.round(clamped)}%`;
        },
        hide() {
            if (el) { el.remove(); el = null; }
        },
    };
})();

window.ProgressRing = ProgressRing;
