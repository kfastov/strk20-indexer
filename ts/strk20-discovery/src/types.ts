export interface ChainProfile {
  name: string;
  chainId: string;
  pool: string;
  genesisBlock: number;
  epochSize: number;
  feedFormat: number;
}
export interface RequestRecord {
  url: string;
  method: "GET" | "POST";
  bytes: number;
  ms: number;
}
export interface Span {
  name: string;
  ms: number;
  bytes?: number;
}
export interface Checkpoint {
  chain_id: string;
  pool: string;
  block_number: number;
  block_hash: string;
  state_root: string;
}
export interface EngineInfo {
  chain_id: string;
  pool: string;
  head: number;
  last_epoch: number | null;
  last_epoch_to: number;
  snapshot_basis: number | null;
  history_floor: number;
  verifiedAt: number | null;
  checkpoint: Checkpoint | null;
  verificationFailed: boolean;
  verified: string;
}
export interface EpochEntry {
  e: number;
  from: number;
  to: number;
  hash: string;
  zst: string;
  bytes: number;
  anchor?: unknown;
}
export interface Manifest {
  v: number;
  chain_id: string;
  pool: string;
  genesis_block: number;
  epoch_size: number;
  head: {
    number: number;
    hash: string;
    l1_accepted: number;
    class: string;
    decode_state: string;
  };
  latest_epoch: number | null;
  epochs: EpochEntry[];
  snapshot?: { e: number; block: number; file: string; bytes: number } | null;
}
export interface WireNote {
  token: string;
  id: string;
  amount: string;
  sender: string;
  spent: boolean;
  knownByBlock: number;
  reportedWriteBlock: number;
  nullifier: string;
  witness: { channelKey: string; index: number; salt: string };
}
export interface WireCursor {
  channels?: Record<
    string,
    {
      channel_key: string;
      last_subchannel_index?: number;
      subchannels?: Record<
        string,
        { last_note_index?: number; total_n_notes?: number }
      >;
    }
  >;
}
export interface DiscoveryResult {
  block: number;
  blockHash: string;
  notes: WireNote[];
  incomingCursor: WireCursor;
  report: {
    incoming_complete: boolean;
    outgoing_complete: boolean;
    history_from: number;
  };
}
export interface ChannelResult {
  block: number;
  total: number;
  channels: {
    recipient: string;
    publicKey: string;
    key: string | null;
    tokens: { token: string; tokenIndex: number; noteNonce: number }[];
  }[];
}
export interface RuntimeOptions {
  network: "mainnet" | "sepolia" | ChainProfile;
  feedUrl: string;
  rpcUrl: string;
  proofRpcUrl: string;
}
/** Actual startup boundaries, not an estimate of elapsed/remaining time. */
export type StartupStage = "engine" | "cache" | "feed" | "checkpoint" | "download" | "verify" | "save" | "ready";
export interface StartupProgress {
  stage: StartupStage;
  completed?: number;
  total?: number;
}
export type WorkerEvent =
  | { event: "startup"; value: StartupProgress }
  | { event: "head"; value: number }
  | { event: "span"; value: Span }
  | { event: "request"; value: RequestRecord }
  | { event: "state"; value: EngineInfo }
  | { event: "error"; value: string };
