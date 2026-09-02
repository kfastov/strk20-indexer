/**
 * A stubbed wasm-pack glue.
 *
 * Not a mock of the thing under test: everything these tests exercise is
 * ADAPTER code (the fetch plan, the identity pin, the error boundary), and
 * stubbing the module is what lets a hand-written manifest be pushed at that
 * code without a feed server or a wasm build.
 *
 * `info` may be given as a string, which is emitted verbatim — that is how a
 * module returning something the adapter cannot parse is simulated.
 */

import type { WasmGlue } from '../src/engine-wasm.ts';

export interface StubOptions {
  info?: Record<string, unknown> | string;
  applied?: Record<string, unknown>;
  staleness?: 'ok' | 'behind' | 'diverged';
}

export interface Stub {
  glue: WasmGlue;
  /** Every genesis a module was constructed over. Empty means none was built. */
  built: string[];
  /** Artifacts handed to `stage_*`, in order. */
  staged: string[];
}

/** A module `info()` with nothing folded: the cold-start shape the planner branches on. */
export const COLD_INFO = {
  chain_id: 'SN_SEPOLIA',
  pool: '0x254a6b2997ef52e9f830ce1f543f6b29768295e8d17e2267d672c552cfe0d91',
  genesis_block: 8271125,
  epoch_size: 10000,
  last_epoch: null,
  last_epoch_hash: null,
  last_epoch_to: 0,
  history_floor: 0,
  snapshot_basis: null,
  head: 0,
  l1_accepted: 0,
  slots: 0,
  tail_generation: 0,
  verified: 'replayed',
  engine_version: '0.0.0-stub',
} as const;

const APPLIED = {
  epochs_applied: 1,
  last_epoch: 827,
  last_epoch_to: 8279999,
  head: 8280100,
  l1_accepted: 8280000,
  tail_rewound: false,
  history_floor: 8270000,
  snapshot_basis: null,
  snapshot_rejected: false,
  state_changed: true,
};

export function stubGlue(opts: StubOptions = {}): Stub {
  const built: string[] = [];
  const staged: string[] = [];
  const info = opts.info ?? COLD_INFO;
  const applied = { ...APPLIED, ...opts.applied };
  /**
   * The real module's `info()` reflects what `apply()` folded, and the adapter
   * depends on that: after the first fold it re-reads `last_epoch` to decide
   * whether the manifest lists epochs the plan did not know to ask for. A stub
   * whose `info()` never moves reports a permanently cold mirror and provokes
   * an endless catch-up round, which is a defect in the stub, not the planner.
   */
  let lastEpoch = typeof info === 'string' ? null : (info['last_epoch'] as number | null);

  class StubEngine {
    constructor(genesisJson: string) {
      built.push(genesisJson);
    }
    static load(_blob: Uint8Array, genesisJson: string) {
      built.push(genesisJson);
      return new StubEngine(genesisJson);
    }
    static version() {
      return '0.0.0-stub';
    }
    info() {
      return typeof info === 'string' ? info : JSON.stringify({ ...info, last_epoch: lastEpoch });
    }
    apply(_coldStart: string) {
      lastEpoch = applied.last_epoch;
      return JSON.stringify(applied);
    }
    check_manifest(_json: string) {
      return opts.staleness ?? 'ok';
    }
    stage_manifest(_json: string) {
      staged.push('manifest');
    }
    stage_epoch(e: bigint, _payload: Uint8Array) {
      staged.push(`epoch:${e}`);
    }
    stage_head(_payload: Uint8Array, _etag: string) {
      staged.push('head');
    }
    stage_anchors(_p: Uint8Array) {
      staged.push('anchors');
    }
    stage_snapshot(e: bigint, _z: Uint8Array, _p: Uint8Array) {
      staged.push(`snapshot:${e}`);
    }
    stage_snapshot_anchor(e: bigint, _j: Uint8Array) {
      staged.push(`snapshot_anchor:${e}`);
    }
    stage_storage_proof() {}
    clear_storage_proofs() {}
    proof_candidates() {
      return '[]';
    }
    discover() {
      return '{}';
    }
    export_state() {
      return new Uint8Array(0);
    }
    forget_owner() {}
    free() {}
  }

  const glue = {
    default: async () => ({ memory: new WebAssembly.Memory({ initial: 1 }) }),
    Engine: StubEngine as unknown as WasmGlue['Engine'],
    set_panic_hook: () => {},
  };
  return { glue: glue as unknown as WasmGlue, built, staged };
}
