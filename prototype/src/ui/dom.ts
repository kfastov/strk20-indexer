/** Four helpers. Everything else is plain DOM, on purpose. */

export function need<T extends Element = HTMLElement>(sel: string, root: ParentNode = document): T {
  const el = root.querySelector<T>(sel);
  if (!el) throw new Error(`missing element: ${sel} — check index.html`);
  return el;
}

export function all<T extends Element = HTMLElement>(sel: string, root: ParentNode = document): T[] {
  return Array.from(root.querySelectorAll<T>(sel));
}

/** Clone the first element of a <template>, typed. */
export function clone<T extends Element = HTMLElement>(templateId: string): T {
  const t = document.getElementById(templateId);
  if (!(t instanceof HTMLTemplateElement)) throw new Error(`missing <template id="${templateId}">`);
  const el = t.content.firstElementChild?.cloneNode(true);
  if (!el) throw new Error(`empty <template id="${templateId}">`);
  return el as T;
}

export function setText(root: ParentNode, sel: string, value: string): void {
  need(sel, root).textContent = value;
}

/** Build a <dl> of label/value rows into an existing container. */
export function renderKv(target: HTMLElement, rows: ReadonlyArray<[string, string, string?]>): void {
  target.replaceChildren();
  for (const [k, v, cls] of rows) {
    const row = clone<HTMLElement>('tpl-kv');
    setText(row, 'dt', k);
    const dd = need('dd', row);
    dd.textContent = v;
    if (cls) dd.className = cls;
    target.append(row);
  }
}

export const prefersReducedMotion = (): boolean =>
  window.matchMedia('(prefers-reduced-motion: reduce)').matches;
