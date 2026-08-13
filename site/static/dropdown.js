/* ═══════════════════════════════════════════════════════════════════════
   Shared dropdown - one select control for the whole site
   ───────────────────────────────────────────────────────────────────────
   A native <select> paints its open list with the operating system, not
   with the page. On a dark site that means light text on a white popup:
   unreadable, and nothing CSS can reach. So every eligible select is
   *enhanced* rather than replaced - the real <select> stays in the DOM as
   the value, and a listbox built from its options is drawn on top.

   Keeping the native element is the point. Page code that reads
   `select.value`, sets it, or listens for `change` carries on working
   untouched; this layer only intercepts the pointing and the painting.

   The open panel is appended to <body> and positioned fixed, so a
   dropdown inside a scrolling card or a modal is never clipped by it.

   Follows the ARIA combobox pattern: focus stays on the trigger and the
   active option is pointed at with aria-activedescendant, which is what
   lets one Tab stop cover the whole control.

   Opt out with `data-no-dropdown` on the select or any ancestor.
   ═══════════════════════════════════════════════════════════════════════ */

(function () {
  'use strict';

  const instances = new WeakMap();
  let seq = 0;
  let open = null;             // the one instance currently showing its panel

  // Type-ahead: matches the native control's behaviour, where letters typed in
  // quick succession build a prefix and a pause starts a new one.
  const TYPE_TIMEOUT = 700;

  function eligible(select) {
    return select instanceof HTMLSelectElement
      && !instances.has(select)
      && !select.multiple
      && select.size <= 1
      // Only a select that is already hidden when we get here. Note this can't
      // be relied on to protect a control that hides itself from script: this
      // file is deferred, so it runs BEFORE DOMContentLoaded and therefore
      // before most page setup. Anything with its own enhancer (the language
      // picker) opts out with data-no-dropdown instead.
      && !select.hidden
      && !select.closest('[data-no-dropdown]');
  }

  function enhance(root) {
    const scope = root || document;
    const list = scope.querySelectorAll ? scope.querySelectorAll('select') : [];
    for (const select of list) {
      if (eligible(select)) attach(select);
    }
  }

  function attach(select) {
    const id = `btt-dd-${++seq}`;
    const wrap = document.createElement('div');
    wrap.className = 'btt-dd';

    const trigger = document.createElement('button');
    trigger.type = 'button';
    trigger.className = 'btt-dd-trigger';
    trigger.id = `${id}-trigger`;
    trigger.setAttribute('role', 'combobox');
    trigger.setAttribute('aria-haspopup', 'listbox');
    trigger.setAttribute('aria-expanded', 'false');
    trigger.setAttribute('aria-controls', `${id}-list`);

    const value = document.createElement('span');
    value.className = 'btt-dd-value';
    const caret = document.createElement('span');
    caret.className = 'btt-dd-caret';
    caret.setAttribute('aria-hidden', 'true');
    trigger.append(value, caret);

    const panel = document.createElement('div');
    panel.className = 'btt-dd-panel';
    panel.id = `${id}-list`;
    panel.setAttribute('role', 'listbox');
    panel.hidden = true;

    // Wear the select's own classes. Nearly every page styles its dropdowns
    // through a class (.up-select, .cdx-select, .gb-select…) and those rules -
    // padding, border, width, font - apply to a button just as well, so the
    // replacement inherits each page's look without touching a line of its CSS.
    // Rules written against the ELEMENT (`.gem-sim select`) can't be inherited
    // this way and name .btt-dd-trigger alongside instead.
    for (const name of select.classList) trigger.classList.add(name);

    select.classList.add('btt-dd-native');
    select.setAttribute('tabindex', '-1');
    select.setAttribute('aria-hidden', 'true');

    select.parentNode.insertBefore(wrap, select);
    // The panel lives here while closed and moves to <body> while open. It has to
    // be in the document from the start, or the trigger's aria-controls points at
    // an element that does not exist yet.
    wrap.append(trigger, select, panel);

    const self = {
      id, select, wrap, trigger, value, panel,
      options: [],          // {option, el, index}
      active: -1,
      typed: '',
      typedAt: 0,
    };
    instances.set(select, self);

    label(self);
    rebuild(self);
    listen(self);
    return self;
  }

  /** Give the trigger the same accessible name the select had. */
  function label(self) {
    const { select, trigger } = self;
    const aria = select.getAttribute('aria-label');
    if (aria) { trigger.setAttribute('aria-label', aria); return; }
    let labelled = select.getAttribute('aria-labelledby');
    if (!labelled) {
      const explicit = select.id
        ? document.querySelector(`label[for="${CSS.escape(select.id)}"]`)
        : null;
      const host = explicit || select.closest('label');
      if (host) {
        if (!host.id) host.id = `${self.id}-label`;
        labelled = host.id;
        // A wrapping <label> would otherwise focus the hidden native select.
        if (!explicit) host.addEventListener('click', (e) => {
          if (!self.wrap.contains(e.target)) { e.preventDefault(); self.trigger.focus(); }
        });
      }
    }
    // Concatenated, not interpolated with a literal space: the minifier eats the
    // gap between two adjacent ${} slots, and this one is a token separator.
    if (labelled) trigger.setAttribute('aria-labelledby', labelled + ' ' + self.id + '-trigger');
    if (select.title) trigger.title = select.title;
  }

  /** Re-read the native options and repaint the panel. */
  function rebuild(self) {
    const { select, panel } = self;
    panel.textContent = '';
    self.options = [];

    const add = (option, container) => {
      if (option.hidden) return;
      const el = document.createElement('div');
      el.className = 'btt-dd-option';
      el.id = `${self.id}-opt-${self.options.length}`;
      el.setAttribute('role', 'option');
      el.textContent = option.textContent;
      if (option.disabled) el.setAttribute('aria-disabled', 'true');
      if (option.title) el.title = option.title;
      el.dataset.index = String(self.options.length);
      self.options.push({ option, el });
      container.appendChild(el);
    };

    for (const child of select.children) {
      if (child.tagName === 'OPTGROUP') {
        if (child.hidden) continue;
        const group = document.createElement('div');
        group.className = 'btt-dd-group';
        group.setAttribute('role', 'group');
        const name = document.createElement('div');
        name.className = 'btt-dd-grouplabel';
        name.textContent = child.label;
        group.appendChild(name);
        for (const option of child.children) add(option, group);
        panel.appendChild(group);
      } else if (child.tagName === 'OPTION') {
        add(child, panel);
      }
    }
    sync(self);
  }

  /** Push the native element's current state into the custom one. */
  function sync(self) {
    const { select, trigger, value } = self;
    const chosen = select.selectedIndex >= 0 ? select.options[select.selectedIndex] : null;
    value.textContent = chosen ? chosen.textContent : '';
    self.wrap.classList.toggle('is-placeholder', !!chosen && chosen.value === '');
    trigger.disabled = select.disabled;
    self.wrap.classList.toggle('is-disabled', select.disabled);
    for (const entry of self.options) {
      const on = entry.option.selected;
      entry.el.setAttribute('aria-selected', String(on));
      entry.el.classList.toggle('is-selected', on);
    }
  }

  function listen(self) {
    const { select, trigger, panel } = self;

    trigger.addEventListener('click', () => (open === self ? close() : show(self)));
    trigger.addEventListener('keydown', (e) => onTriggerKey(self, e));

    // Pointer, not click: matching the native control means the list commits on
    // release, so press-drag-release over an option selects it in one gesture.
    panel.addEventListener('pointerup', (e) => {
      const el = e.target.closest('.btt-dd-option');
      if (el && el.getAttribute('aria-disabled') !== 'true') {
        choose(self, Number(el.dataset.index));
      }
    });
    panel.addEventListener('pointermove', (e) => {
      const el = e.target.closest('.btt-dd-option');
      if (el) mark(self, Number(el.dataset.index), false);
    });

    // Anything that changes the select from the outside - page code setting
    // .value, a fresh set of options, or i18n re-translating the labels.
    select.addEventListener('change', () => sync(self));
    const watcher = new MutationObserver(() => rebuild(self));
    watcher.observe(select, {
      childList: true, subtree: true, characterData: true,
      attributes: true, attributeFilter: ['disabled', 'value'],
    });
  }

  function onTriggerKey(self, e) {
    const key = e.key;
    if (open !== self) {
      if (key === 'ArrowDown' || key === 'ArrowUp' || key === 'Enter' || key === ' ') {
        e.preventDefault();
        show(self);
        return;
      }
      // Typing on a closed control jumps selection, exactly like the native one.
      if (key.length === 1 && !e.metaKey && !e.ctrlKey && !e.altKey) {
        const hit = typeahead(self, key);
        if (hit >= 0) { e.preventDefault(); commit(self, hit); }
      }
      return;
    }
    switch (key) {
      case 'ArrowDown': e.preventDefault(); step(self, 1); break;
      case 'ArrowUp': e.preventDefault(); step(self, -1); break;
      case 'Home': e.preventDefault(); mark(self, firstEnabled(self, 0, 1), true); break;
      case 'End': e.preventDefault(); mark(self, firstEnabled(self, self.options.length - 1, -1), true); break;
      case 'Enter': case ' ':
        e.preventDefault();
        if (self.active >= 0) choose(self, self.active); else close();
        break;
      case 'Escape': e.preventDefault(); close(true); break;
      case 'Tab': close(); break;             // let focus move on naturally
      default:
        if (key.length === 1 && !e.metaKey && !e.ctrlKey && !e.altKey) {
          const hit = typeahead(self, key);
          if (hit >= 0) { e.preventDefault(); mark(self, hit, true); }
        }
    }
  }

  function typeahead(self, char) {
    const now = Date.now();
    self.typed = (now - self.typedAt > TYPE_TIMEOUT ? '' : self.typed) + char.toLowerCase();
    self.typedAt = now;
    const from = self.active >= 0 ? self.active : self.select.selectedIndex;
    const count = self.options.length;
    // A repeated single letter cycles through the entries starting with it.
    const repeat = self.typed.length > 1 && /^(.)\1+$/.test(self.typed);
    const needle = repeat ? self.typed[0] : self.typed;
    for (let i = 1; i <= count; i++) {
      const at = (from + (repeat || self.typed.length === 1 ? i : 0) + count) % count;
      const entry = self.options[at];
      if (!entry || entry.option.disabled) continue;
      if (entry.el.textContent.trim().toLowerCase().startsWith(needle)) return at;
      if (!repeat && self.typed.length > 1) break;   // fall through to a full scan
    }
    if (!repeat && self.typed.length > 1) {
      for (let i = 0; i < count; i++) {
        const entry = self.options[i];
        if (entry.option.disabled) continue;
        if (entry.el.textContent.trim().toLowerCase().startsWith(needle)) return i;
      }
    }
    return -1;
  }

  function firstEnabled(self, from, direction) {
    for (let i = from; i >= 0 && i < self.options.length; i += direction) {
      if (!self.options[i].option.disabled) return i;
    }
    return self.active;
  }

  function step(self, direction) {
    const start = self.active < 0 ? self.select.selectedIndex : self.active;
    for (let i = start + direction; i >= 0 && i < self.options.length; i += direction) {
      if (!self.options[i].option.disabled) { mark(self, i, true); return; }
    }
  }

  function mark(self, index, scroll) {
    if (index < 0 || index >= self.options.length) return;
    if (self.active >= 0 && self.options[self.active]) {
      self.options[self.active].el.classList.remove('is-active');
    }
    self.active = index;
    const el = self.options[index].el;
    el.classList.add('is-active');
    self.trigger.setAttribute('aria-activedescendant', el.id);
    if (scroll) el.scrollIntoView({ block: 'nearest' });
  }

  function choose(self, index) {
    commit(self, index);
    close();
  }

  /** Write the choice back to the native element and tell the page about it. */
  function commit(self, index) {
    const entry = self.options[index];
    if (!entry || entry.option.disabled) return;
    if (entry.option.selected) { sync(self); return; }
    entry.option.selected = true;
    sync(self);
    // `input` then `change`, in the order a real control fires them.
    self.select.dispatchEvent(new Event('input', { bubbles: true }));
    self.select.dispatchEvent(new Event('change', { bubbles: true }));
  }

  function show(self) {
    if (open) close();
    open = self;
    self.panel.hidden = false;
    document.body.appendChild(self.panel);
    self.trigger.setAttribute('aria-expanded', 'true');
    self.wrap.classList.add('is-open');
    place(self);
    const selected = self.select.selectedIndex;
    mark(self, selected >= 0 ? selected : firstEnabled(self, 0, 1), true);
    window.addEventListener('scroll', reposition, true);
    window.addEventListener('resize', reposition);
    document.addEventListener('pointerdown', onOutside, true);
  }

  function close(restoreFocus) {
    if (!open) return;
    const self = open;
    open = null;
    self.panel.hidden = true;
    if (self.panel.parentNode === document.body) self.panel.remove();
    self.wrap.appendChild(self.panel);
    self.trigger.setAttribute('aria-expanded', 'false');
    self.trigger.removeAttribute('aria-activedescendant');
    self.wrap.classList.remove('is-open');
    if (self.active >= 0 && self.options[self.active]) {
      self.options[self.active].el.classList.remove('is-active');
    }
    self.active = -1;
    self.typed = '';
    window.removeEventListener('scroll', reposition, true);
    window.removeEventListener('resize', reposition);
    document.removeEventListener('pointerdown', onOutside, true);
    if (restoreFocus) self.trigger.focus();
  }

  function onOutside(e) {
    if (!open) return;
    if (open.wrap.contains(e.target) || open.panel.contains(e.target)) return;
    close();
  }

  function reposition(e) {
    if (!open) return;
    // Scrolling the list itself is not the page moving under the panel.
    if (e && e.target && (e.target === open.panel || open.panel.contains(e.target))) return;
    place(open);
  }

  /** Pin the panel under (or over) its trigger, in viewport coordinates. */
  function place(self) {
    const box = self.trigger.getBoundingClientRect();
    const panel = self.panel;
    const margin = 8;
    panel.style.minWidth = `${box.width}px`;
    // Measuring the natural height means dropping the cap, and an uncapped
    // panel has nothing to scroll - the browser clamps scrollTop to 0 on the
    // way past. Carry it over, or every measure scrolls the list back to top.
    const scrolled = panel.scrollTop;
    panel.style.maxHeight = '';
    const height = panel.offsetHeight;
    const below = window.innerHeight - box.bottom - margin;
    const above = box.top - margin;
    // Drop upwards only when there is genuinely more room up there.
    const up = height > below && above > below;
    const room = Math.max(120, up ? above : below);
    panel.style.maxHeight = `${room}px`;
    panel.style.left = `${Math.max(margin, Math.min(box.left, window.innerWidth - panel.offsetWidth - margin))}px`;
    panel.style.top = up ? `${Math.max(margin, box.top - Math.min(height, room) - 4)}px`
                         : `${box.bottom + 4}px`;
    panel.scrollTop = scrolled;
  }

  // ─── Boot ──────────────────────────────────────────────────────────
  // Selects appear long after load on most of these pages (a filter list built
  // from a fetch, a modal opened on demand), so watch for them rather than
  // asking every caller to remember a refresh.
  function start() {
    enhance(document);
    new MutationObserver((records) => {
      for (const record of records) {
        for (const node of record.addedNodes) {
          if (node.nodeType !== 1) continue;
          if (node.tagName === 'SELECT') { if (eligible(node)) attach(node); }
          else enhance(node);
        }
      }
    }).observe(document.documentElement, { childList: true, subtree: true });
  }

  if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', start);
  } else {
    start();
  }

  window.BTTDropdown = {
    enhance,
    /** Repaint one select's dropdown after changing its options from code. */
    refresh(select) {
      const self = instances.get(select);
      if (self) rebuild(self);
      else if (eligible(select)) attach(select);
    },
    close: () => close(),
  };
})();
