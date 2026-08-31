/**
 * The demo's identities.
 *
 * demo-app.md §4 Stage 2:
 *   - a GENERATED key discovers nothing (it is nobody's key), and it is useful
 *     precisely because it proves the request stream is identical to a real
 *     key's;
 *   - PASTE is not enabled in the published build. A page under our name that
 *     asks you to paste a wallet secret teaches exactly the behaviour that gets
 *     our users phished later. A viewing key is read-only, so this is not a
 *     spend risk — it is a habit risk, and the mitigation costs a build flag.
 *
 * The key never leaves this module's closures: `Account.viewingKey()` hands out
 * a FRESH copy per call because the client zeroizes what it is given, and no
 * code path renders it.
 */

import type { Account } from 'strk20-discovery';
import FIXTURE from '../fixtures/replay-identities.json' with { type: 'json' };

export const ALLOW_PASTE = import.meta.env.DEV;

function hexToBytes(hex: string): Uint8Array {
  const out = new Uint8Array(hex.length / 2);
  for (let i = 0; i < out.length; i++) out[i] = parseInt(hex.slice(i * 2, i * 2 + 2), 16);
  return out;
}

export interface DemoIdentity {
  id: 'A' | 'B' | 'generated';
  label: string;
  account: Account;
  /** For the in-page scanner. Never rendered, never logged, never sent. */
  secretBytes: Uint8Array;
  /** Discovery is expected to find something for this identity. */
  ownsNoteInFixture: boolean;
}

function make(
  id: DemoIdentity['id'],
  label: string,
  address: `0x${string}`,
  key: Uint8Array,
  ownsNoteInFixture: boolean,
): DemoIdentity {
  const held = key;
  return {
    id,
    label,
    ownsNoteInFixture,
    secretBytes: held,
    account: {
      address,
      viewingKey: async () => Uint8Array.from(held),
    },
  };
}

export function identityA(): DemoIdentity {
  return make(
    'A',
    FIXTURE.A.label,
    FIXTURE.A.address as `0x${string}`,
    hexToBytes(FIXTURE.A.viewingKey),
    true,
  );
}

export function identityB(): DemoIdentity {
  return make(
    'B',
    FIXTURE.B.label,
    FIXTURE.B.address as `0x${string}`,
    hexToBytes(FIXTURE.B.viewingKey),
    false,
  );
}

/** Nobody's key. It will find nothing, and the demo says so before you click. */
export function generatedIdentity(): DemoIdentity {
  const key = new Uint8Array(32);
  crypto.getRandomValues(key);
  const addr = new Uint8Array(31);
  crypto.getRandomValues(addr);
  const address = ('0x' +
    Array.from(addr, (b) => b.toString(16).padStart(2, '0')).join('')) as `0x${string}`;
  return make('generated', 'generated key (owns nothing, by construction)', address, key, false);
}

/**
 * The address is a PUBLIC value the user is looking at in their own browser, so
 * it is logged. It is separately asserted absent from every request record,
 * which is a different and stronger claim than not printing it.
 */
export function shortAddress(a: string): string {
  return a.length > 14 ? `${a.slice(0, 8)}…${a.slice(-4)}` : a;
}
