import { Strk20Error } from "./errors.js";
/**
 * For a backend or CLI that legitimately holds the bytes for the process
 * lifetime. Named so the shape is visible in the integrator's own review.
 *
 * This holds a JS buffer that cannot be reliably zeroized. The guarantee this
 * package makes is NON-TRANSMISSION, not host memory hygiene — see the README
 * paragraph that says the module never writes a key anywhere.
 */
export function staticAccount(address, key) {
    assertKey(key);
    const held = Uint8Array.from(key);
    return {
        address,
        // A FRESH copy per call, because the client zeroizes what it is given.
        viewingKey: async () => Uint8Array.from(held),
    };
}
export function assertKey(key) {
    if (!(key instanceof Uint8Array) || key.length !== 32) {
        throw new Strk20Error('KEY_INVALID', 'viewing key must be exactly 32 bytes', {
            got: key instanceof Uint8Array ? key.length : typeof key,
        });
    }
}
export function assertAddress(a) {
    if (!/^0x[0-9a-fA-F]{1,64}$/.test(a)) {
        throw new Strk20Error('CONFIG_INVALID', 'address must be 0x-prefixed hex', { option: 'address' });
    }
}
/** Zeroize a buffer we were handed. Called on every path out of a pass. */
export function zeroize(b) {
    if (b && b.length > 0 && !isDetached(b))
        b.fill(0);
}
function isDetached(b) {
    try {
        return b.byteLength === 0 && b.buffer.byteLength === 0;
    }
    catch {
        return true;
    }
}
//# sourceMappingURL=account.js.map