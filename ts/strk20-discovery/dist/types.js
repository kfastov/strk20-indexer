/**
 * Public type surface. Transcribed from consumer-path.md §4.2; where this file
 * deviates the deviation is commented and carries a reason.
 *
 * Every union here is CLOSED on purpose (§4.2 "Must not ship"): a logger or a
 * telemetry pipe attached to our event bus cannot receive key material, because
 * no member of `DiscoveryEvent` is typed as an open `string` / `unknown` that
 * could carry one. `NotesResult.raw` is the single exception and it is the
 * module's own `SyncReport`, which the Rust scanner asserts key-clean.
 */
export {};
//# sourceMappingURL=types.js.map