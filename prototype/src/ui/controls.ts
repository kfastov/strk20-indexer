/**
 * The bottom bar: three staged actions plus the discovery controls.
 *
 * "Staged like an old swap UI where approve precedes swap" is a visual claim,
 * not just a disabled attribute — so each action renders as a numbered strip
 * whose step 1 lights, completes and hands over to step 2, and every locked
 * control prints WHY it is locked underneath instead of going quietly grey.
 */

import type { ActionKind } from '../engine/chain';
import { actionGate, stepGate, stepVisual, type AppState } from '../state';
import { all, need } from './dom';

const KINDS: readonly ActionKind[] = ['deposit', 'send', 'withdraw'];

export class Controls {
  private checkNow = need<HTMLButtonElement>('#btn-check-now');
  private cancelWait = need<HTMLButtonElement>('#btn-cancel-wait');
  private auxSub = need('#aux-sub');
  private auxGate = need('#aux-gate');

  render(s: AppState): void {
    for (const kind of KINDS) this.renderAction(s, kind);
    this.renderAux(s);
    this.renderRunButtons(s);
  }

  private renderAction(s: AppState, kind: ActionKind): void {
    const root = need(`.action[data-action="${kind}"]`);
    const gate = actionGate(s, kind);
    const armed = s.action.kind === kind;
    const active = s.stage.s === 'acting' && s.stage.kind === kind;
    const waiting = s.stage.s === 'waiting' && s.stage.kind === kind;

    root.dataset['state'] = waiting
      ? 'waiting'
      : active
        ? 'active'
        : armed && s.action.stepDone
          ? 'armed'
          : gate.enabled
            ? 'ready'
            : 'locked';

    for (const step of [0, 1] as const) {
      const btn = need<HTMLButtonElement>(
        `.step-btn[data-action="${kind}"][data-step="${step}"]`,
        root,
      );
      const g = stepGate(s, kind, step);
      btn.disabled = !g.enabled;
      btn.dataset['visual'] = stepVisual(s, kind, step);
      btn.setAttribute('aria-describedby', `gate-${kind}`);
    }

    const gateEl = need('[data-role="gate"]', root);
    gateEl.id = `gate-${kind}`;
    if (waiting) {
      gateEl.textContent = 'submitted';
      gateEl.dataset['tone'] = 'pending';
    } else if (active) {
      gateEl.textContent = 'running';
      gateEl.dataset['tone'] = 'pending';
    } else if (armed && s.action.stepDone) {
      gateEl.textContent = 'step 2 unlocked';
      gateEl.dataset['tone'] = 'ok';
    } else if (!gate.enabled) {
      gateEl.textContent = gate.reason;
      gateEl.dataset['tone'] = 'locked';
    } else {
      gateEl.textContent = gate.reason;
      gateEl.dataset['tone'] = 'ready';
    }
  }

  private renderAux(s: AppState): void {
    // A check that finds nothing writes no log line, so the only feedback that
    // the button did anything is the button itself.
    const canCheck = s.stage.s === 'ready' || s.stage.s === 'waiting';
    this.checkNow.hidden = s.subscription || !canCheck;
    this.checkNow.disabled = !canCheck || s.checking;
    this.checkNow.textContent = s.checking ? 'checking…' : 'check now';
    this.cancelWait.hidden = s.stage.s !== 'waiting';

    this.auxSub.textContent = s.subscription ? 'auto' : 'manual';

    if (s.stage.s === 'waiting') {
      this.auxGate.textContent = s.subscription ? 'waiting' : 'check now';
      this.auxGate.dataset['tone'] = 'pending';
    } else if (s.stage.s === 'ready') {
      this.auxGate.textContent = s.subscription ? 'listening' : 'idle';
      this.auxGate.dataset['tone'] = 'ready';
    } else {
      this.auxGate.textContent = '';
      this.auxGate.dataset['tone'] = 'locked';
    }
  }

  private renderRunButtons(s: AppState): void {
    const busy = s.stage.s === 'syncing' || s.stage.s === 'acting' || s.stage.s === 'boot';
    const cold = need<HTMLButtonElement>('#btn-run-cold');
    const warm = need<HTMLButtonElement>('#btn-run-warm');
    cold.disabled = busy;
    warm.disabled = busy || s.stage.s === 'cold';

    for (const b of all<HTMLButtonElement>('.lane')) b.disabled = busy;
    for (const b of all<HTMLButtonElement>('.ident')) b.disabled = busy;
  }
}
