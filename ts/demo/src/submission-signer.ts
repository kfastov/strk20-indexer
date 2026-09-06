import { Signer, type Signature } from "starknet";

/** Keep the exact SDK-computed transaction hash before signing can enable a send. */
export class SubmissionSigner extends Signer {
  private readonly beforeSignature: (hash: string) => Promise<void>;
  constructor(key: string, beforeSignature: (hash: string) => Promise<void>) {
    super(key);
    this.beforeSignature = beforeSignature;
  }
  protected override async signRaw(hash: string): Promise<Signature> {
    await this.beforeSignature(hash);
    return super.signRaw(hash);
  }
}
