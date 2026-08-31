/**
 * strk20-discovery — package root.
 *
 * `LocalDiscoveryProvider` is first because our customer's integration is one
 * field in `createPrivateTransfers({ discoveryProvider })`.
 *
 * `DelegatedClient` is deliberately NOT here. It lives at
 * `strk20-discovery/delegated`, because in delegated mode the viewing key
 * leaves the browser and that should not be one autocomplete away.
 */
export { LocalDiscoveryProvider, type LocalDiscoveryProviderOptions } from './provider.ts';
export { KeylessClient, type KeylessClientOptions } from './client.ts';
export { staticAccount } from './account.ts';
export { Strk20Error, isStrk20Error, type Strk20ErrorCode, type ErrorDetail } from './errors.ts';
export { MAINNET, SEPOLIA, resolveProfile } from './profiles.ts';
export { FEED_PATH_ALLOWLIST, isAllowedFeedPath } from './net.ts';
export { keyId } from './kdf.ts';
export { MemoryStorage, IdbStorage, dbName, deleteDatabase, openStorage, type StorageAdapter, type StateMeta, } from './storage.ts';
export type { Engine, EngineFactory, Step, StepFetch, StepDone, StepRpc, EngineInfo } from './engine.ts';
export { ENCODINGS_FIXTURE_V1, encodingsFixtureDigest, encodeAll, scan, surfacesOfRequest, type EncodingName, type ScanHit, type ScanSecret, type ScanSurface, } from './scan.ts';
export type { Account, ChainProfile, ClientStatus, DiscoveryClient, DiscoveryEvent, DiscoveryProvider, FeedState, HistoryTx, NetworkSummary, Note, NotesResult, Phase, Progress, RequestArtifact, RequestRecord, Subscription, SyncTiming, } from './types.ts';
//# sourceMappingURL=index.d.ts.map