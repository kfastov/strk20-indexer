/** The actual WASM ABI, without simulated stepping or ignored budgets. */
export interface Engine {
  stage_manifest(json: string): void;
  stage_epoch(epoch: bigint, payload: Uint8Array): void;
  stage_snapshot(
    epoch: bigint,
    compressed: Uint8Array,
    payload: Uint8Array,
  ): void;
  stage_head(payload: Uint8Array, etag: string): void;
  stage_checkpoint(checkpoint: string, proof: string): void;
  apply(mode: string): string;
  info(): string;
  export_state(): Uint8Array;
  cache_revision(): number;
  discover(owner: string, key: Uint8Array): string;
  channels(owner: string, key: Uint8Array, recipients: string): string;
  requirement(
    owner: string,
    key: Uint8Array,
    recipient: string,
    token: string,
  ): number;
  forget_owner(owner: string): void;
  free(): void;
}
export interface EngineModule {
  Engine: {
    new (genesis: string): Engine;
    load(blob: Uint8Array, genesis: string): Engine;
  };
}
