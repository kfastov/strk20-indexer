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
export { LocalDiscoveryProvider } from "./provider.js";
export { KeylessClient } from "./client.js";
export { staticAccount } from "./account.js";
export { Strk20Error, isStrk20Error } from "./errors.js";
export { MAINNET, SEPOLIA, resolveProfile } from "./profiles.js";
export { FEED_PATH_ALLOWLIST, isAllowedFeedPath } from "./net.js";
export { keyId } from "./kdf.js";
export { MemoryStorage, IdbStorage, dbName, deleteDatabase, openStorage, } from "./storage.js";
export { ENCODINGS_FIXTURE_V1, encodingsFixtureDigest, encodeAll, scan, surfacesOfRequest, } from "./scan.js";
//# sourceMappingURL=index.js.map