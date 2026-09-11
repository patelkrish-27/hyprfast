(() => {
  if (window.__hyprfastHint) return;
  const CHARSET = ['A','S','D','F','G','H','J','K','L','Q','W','E','R','T','Y','U','I','O','P','Z','X','C','V','B','N','M'];
  const SELECTOR = 'a, button, input, select, textarea, [role="button"], [role="link"], [role="textbox"], [role="combobox"], [role="checkbox"], [role="radio"], [role="tab"], [role="menuitem"], [onclick], [contenteditable], [draggable="true"], summary, [tabindex]:not([tabindex="-1"])';
  const CONTAINER_ID = '__hyprfast-hint-container';
  let labelToEl = new Map();

  function genLabels(count) {
    const n = CHARSET.length;
    const out = [];
    for (let i = 0; i < count; i++) {
      if (i < n) {
        out.push(CHARSET[i]);
      } else {
        let rem = i - n;
        if (rem < n * n) {
          const a = Math.floor(rem / n);
          const b = rem % n;
          out.push(CHARSET[a] + CHARSET[b]);
        } else {
          rem -= n * n;
          // triple: 26*26*26 = 17576
          if (rem < n * n * n) {
            const a = Math.floor(rem / (n * n));
            const bc = rem % (n * n);
            const b = Math.floor(bc / n);
            const c = bc % n;
            out.push(CHARSET[a] + CHARSET[b] + CHARSET[c]);
          } else {
            // quad fallback (rare)
            rem -= n * n * n;
            const a = Math.floor(rem / (n * n * n)) % n;
            const b = Math.floor(rem / (n * n)) % n;
            const c = Math.floor(rem / n) % n;
            const d = rem % n;
            out.push(CHARSET[a] + CHARSET[b] + CHARSET[c] + CHARSET[d]);
          }
        }
      }
    }
    return out;
  }

  function isVisible(el) {
    const rect = el.getBoundingClientRect();
    if (rect.width <= 0 || rect.height <= 0) return false;
    const style = window.getComputedStyle(el);
    if (style.visibility === 'hidden' || style.display === 'none' || style.opacity === '0') return false;
    if (style.pointerEvents === 'none') return false;
    // in viewport
    if (rect.bottom < 0 || rect.right < 0 || rect.top > window.innerHeight || rect.left > window.innerWidth) return false;
    // hidden via hidden attribute
    if (el.hidden) return false;
    return true;
  }

  function uniqueSelector(el) {
    if (el.id) return '#' + CSS.escape(el.id);
    // try to build selector with tag + nth-of-type if needed
    let sel = el.tagName.toLowerCase();
    if (el.className && typeof el.className === 'string') {
      const cls = el.className.trim().split(/\s+/).filter(Boolean).slice(0,2).map(c=> '.'+CSS.escape(c)).join('');
      if (cls) sel += cls;
    }
    // ensure uniqueness via query count
    try {
      const all = document.querySelectorAll(sel);
      if (all.length === 1) return sel;
      // fallback to nth-child path
      const parts = [];
      let cur = el;
      while (cur && cur !== document.body && cur !== document.documentElement) {
        let tag = cur.tagName.toLowerCase();
        if (cur.id) { parts.unshift('#' + CSS.escape(cur.id)); break; }
        let idx = 1;
        let sib = cur.previousElementSibling;
        while (sib) { if (sib.tagName === cur.tagName) idx++; sib = sib.previousElementSibling; }
        parts.unshift(tag + ':nth-of-type(' + idx + ')');
        cur = cur.parentElement;
        if (parts.length > 4) break;
      }
      return parts.join(' > ');
    } catch(e) {
      return el.tagName.toLowerCase();
    }
  }

  function textFor(el) {
    const v = (el.getAttribute('aria-label') || el.innerText || el.value || el.placeholder || el.getAttribute('alt') || '').trim();
    return v.slice(0,120);
  }

  function roleFor(el, tag) {
    return el.getAttribute('role') || ({a:'link',button:'button',input:'textbox',select:'combobox',textarea:'textbox',summary:'button'}[tag] || tag);
  }

  function clearHints() {
    const c = document.getElementById(CONTAINER_ID);
    if (c) c.remove();
    labelToEl.clear();
  }

  function scanAndOverlay() {
    clearHints();
    const candidates = Array.from(document.querySelectorAll(SELECTOR));
    // shadow DOM piercing: walk into open shadow roots and collect candidates there
    (function collectShadow(root, acc) {
      const hosts = root.querySelectorAll('*');
      for (const host of hosts) {
        if (host.shadowRoot) {
          const shadowCands = host.shadowRoot.querySelectorAll(SELECTOR);
          for (const c of shadowCands) acc.push(c);
          collectShadow(host.shadowRoot, acc);
        }
      }
    })(document, candidates);
    const visible = candidates.filter(isVisible);
    // stable DOM order is already document order; ensure deterministic
    const count = visible.length;
    const labels = genLabels(count);
    const container = document.createElement('div');
    container.id = CONTAINER_ID;
    container.style.cssText = 'position:absolute; left:0; top:0; width:0; height:0; z-index:2147483647; pointer-events:none;';
    document.documentElement.appendChild(container);

    const out = [];
    for (let i = 0; i < visible.length; i++) {
      const el = visible[i];
      const label = labels[i];
      labelToEl.set(label, el);
      const rect = el.getBoundingClientRect();
      const tag = el.tagName.toLowerCase();
      // overlay span
      const span = document.createElement('span');
      span.setAttribute('data-hint', label);
      span.textContent = label;
      // absolute positioned relative to viewport -> use fixed, or absolute with scroll offsets
      const scrollX = window.scrollX || window.pageXOffset;
      const scrollY = window.scrollY || window.pageYOffset;
      span.style.cssText = 'position:absolute; left:' + (rect.left + scrollX) + 'px; top:' + (rect.top + scrollY) + 'px; '
        + 'background:#ffeb3b; color:#000; border:1px solid #000; border-radius:2px; '
        + 'font:10px/10px monospace; padding:1px 2px; z-index:2147483647; pointer-events:none; '
        + 'transform:translate(-2px,-8px);';
      container.appendChild(span);

      out.push({
        label: label,
        tag: tag,
        role: roleFor(el, tag),
        name: textFor(el) || tag,
        rect: { x: Math.round(rect.x), y: Math.round(rect.y), width: Math.round(rect.width), height: Math.round(rect.height) },
        selector: uniqueSelector(el),
        text: textFor(el)
      });
    }
    return out;
  }

  function clickLabel(label) {
    const el = labelToEl.get(label);
    if (!el) return {error: 'label not found: ' + label, labels: Array.from(labelToEl.keys())};
    // ensure still attached and visible
    if (!document.contains(el)) return {error: 'element detached'};
    el.scrollIntoView({block:'center', inline:'center', behavior:'instant'});
    // focus first then click
    try { el.focus({preventScroll:true}); } catch(e) {}
    el.click();
    // also dispatch mouse events for frameworks that listen
    try {
      const r = el.getBoundingClientRect();
      const opts = {bubbles:true, cancelable:true, view:window, clientX: r.left + r.width/2, clientY: r.top + r.height/2};
      el.dispatchEvent(new MouseEvent('mousedown', opts));
      el.dispatchEvent(new MouseEvent('mouseup', opts));
    } catch(e) {}
    return {clicked:true, label: label, tag: el.tagName};
  }

  function focusAndType(label, text) {
    const el = labelToEl.get(label);
    if (!el) return {error: 'label not found: ' + label};
    if (!document.contains(el)) return {error: 'element detached'};
    el.scrollIntoView({block:'center', inline:'center', behavior:'instant'});
    try { el.focus({preventScroll:true}); } catch(e) { el.focus(); }
    if (el.isContentEditable) {
      // select all and insert
      const sel = window.getSelection();
      const range = document.createRange();
      range.selectNodeContents(el);
      sel.removeAllRanges();
      sel.addRange(range);
      document.execCommand('insertText', false, text);
      return {typed: text.length, label: label, tag: el.tagName, via: 'contenteditable'};
    } else if (el.tagName === 'INPUT' || el.tagName === 'TEXTAREA' || el.tagName === 'SELECT') {
      // handle select separately? For select we try to set value
      if (el.tagName === 'SELECT') {
        // try to find option matching text
        let found = false;
        for (const opt of el.options) {
          if (opt.value === text || opt.text === text || opt.text.trim() === text.trim()) {
            opt.selected = true; found = true; break;
          }
        }
        if (!found && el.options.length>0) {
          // fallback set value
          el.value = text;
        }
        el.dispatchEvent(new Event('input', {bubbles:true}));
        el.dispatchEvent(new Event('change', {bubbles:true}));
        return {typed: text.length, label: label, tag: el.tagName, via: 'select'};
      }
      el.value = text;
      el.dispatchEvent(new Event('input', {bubbles:true}));
      el.dispatchEvent(new Event('change', {bubbles:true}));
      // also dispatch keyboard events for react controlled inputs
      el.dispatchEvent(new KeyboardEvent('keydown', {key:'a', bubbles:true}));
      return {typed: text.length, label: label, tag: el.tagName};
    } else {
      // generic focusable: try to set value if exists or innerText
      if ('value' in el) {
        el.value = text;
        el.dispatchEvent(new Event('input', {bubbles:true}));
        el.dispatchEvent(new Event('change', {bubbles:true}));
        return {typed: text.length, label: label, tag: el.tagName, via:'value'};
      } else {
        el.textContent = text;
        return {typed: text.length, label: label, tag: el.tagName, via:'textContent'};
      }
    }
  }

  // remove on navigation
  window.addEventListener('beforeunload', clearHints, {once:false});
  window.addEventListener('pagehide', clearHints);
  window.addEventListener('popstate', clearHints);
  window.addEventListener('hashchange', clearHints);
  if (window.navigation) {
    try { window.navigation.addEventListener('navigate', clearHints); } catch(e){}
  }
  // SPA mutation: if location href changes without events, snapshot() will clear anyway

  window.__hyprfastHint = {
    snapshot: scanAndOverlay,
    click: clickLabel,
    focusAndType: focusAndType,
    clear: clearHints,
    _labels: genLabels,
    _charset: CHARSET
  };
})();
