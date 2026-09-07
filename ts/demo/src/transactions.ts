import type { LocalDiscoveryProvider } from "strk20-discovery";
import {
  Account,
  CallData,
  RpcProvider,
  cairo,
  type Call,
  type UniversalDetails,
} from "starknet";
import {
  createPrivateTransfers,
  ProvingServiceProofProvider,
  type DiscoveryProviderInterface,
} from "strk20-discovery/privacy-sdk";
import { STRK } from "./network.ts";
import { saveWallet, type Wallet, type Action } from "./wallet.ts";
import { Operations, type Operation } from "./operations.ts";
import { SubmissionSigner } from "./submission-signer.ts";
import { networkConfig } from "./rpc-settings.ts";
import { waitForReceipt } from "./confirmation.ts";
import { waitForBlock } from "./block-wait.ts";

const PROOF_DEPTH = 9;
type TransactionDiscovery = DiscoveryProviderInterface & Pick<LocalDiscoveryProvider, "atBlock">;
export class Transactions {
  readonly rpc: RpcProvider;
  private readonly account: Account;
  private readonly transfers;
  private readonly discovery: TransactionDiscovery;
  private readonly wallet: Wallet;
  private readonly operations: Operations;
  private submission: Operation | undefined;
  constructor(
    wallet: Wallet,
    discovery: TransactionDiscovery,
    operations: Operations,
  ) {
    this.wallet = wallet;
    this.operations = operations;
    this.discovery = discovery;
    const config = networkConfig(wallet.network);
    this.rpc = new RpcProvider({ nodeUrl: config.rpc, batch: false });
    this.account = new Account({
      provider: this.rpc,
      address: wallet.address,
      signer: new SubmissionSigner(wallet.signingKey, async (hash) => {
        // Fee estimation and prover authentication happen before this is armed.
        if (!this.submission) return;
        const pending = this.wallet.pending;
        if (!pending) throw new Error("Missing submission intent.");
        if (pending.hash && BigInt(pending.hash) !== BigInt(hash))
          throw new Error("The SDK attempted to change a pending transaction.");
        const previous = pending.hash;
        pending.hash = hash;
        try {
          await saveWallet(this.wallet);
        } catch (error) {
          if (previous) pending.hash = previous;
          else delete pending.hash;
          throw error;
        }
        this.submission.detail = `${config.explorer}/tx/${hash}`;
      }),
      cairoVersion: "1",
    });
    this.transfers = this.makeTransfers(discovery);
  }
  private makeTransfers(discovery: DiscoveryProviderInterface) {
    const config = networkConfig(this.wallet.network);
    return createPrivateTransfers({
      account: this.account,
      poolContractAddress: config.pool,
      viewingKeyProvider: {
        getViewingKey: async () => BigInt(this.wallet.viewingKey),
      },
      discoveryProvider: discovery,
      provingProvider: new ProvingServiceProofProvider(
        config.prover,
        config.chainId,
        { requestTimeoutMs: 180_000 },
      ),
    });
  }
  async verifyNetwork(): Promise<void> {
    if ((await this.rpc.getChainId()) !== networkConfig(this.wallet.network).chainId)
      throw new Error("RPC reports another network.");
    await this.rpc.getClass(this.wallet.classHash, "latest");
  }
  async balance(): Promise<bigint> {
    const [lo, hi] = await this.rpc.callContract(
      {
        contractAddress: STRK,
        entrypoint: "balance_of",
        calldata: [this.wallet.address],
      },
      "latest",
    );
    return BigInt(lo!) + (BigInt(hi!) << 128n);
  }
  async deployed(): Promise<boolean> {
    try {
      await this.rpc.getNonceForAddress(this.wallet.address, "latest");
      return true;
    } catch (e) {
      if (/CONTRACT_NOT_FOUND|Contract not found|\b20\b/.test(String(e)))
        return false;
      throw e;
    }
  }
  async poolFee(): Promise<bigint> {
    const [fee] = await this.rpc.callContract(
      {
        contractAddress: this.wallet.pool,
        entrypoint: "get_fee_amount",
        calldata: [],
      },
      "latest",
    );
    return BigInt(fee!);
  }
  async resume(): Promise<{ hash: string; block: number } | undefined> {
    if (!this.wallet.pending) return;
    if (!this.wallet.pending.hash)
      throw new Error(
        "The previous submission has an unknown outcome. Check this address in the explorer before sending again.",
      );
    return this.receipt(this.wallet.pending.action, this.wallet.pending.hash);
  }
  private async receipt(
    action: Action,
    hash: string,
  ): Promise<{ hash: string; block: number }> {
    const receipt = await waitForReceipt(
      hash, networkConfig(this.wallet.network).ws, () => this.rpc.getTransactionReceipt(hash),
    );
    if (receipt.execution_status === "REVERTED") {
      delete this.wallet.pending;
      await saveWallet(this.wallet);
      throw new Error(
        "Transaction reverted. Gas was charged; inspect the explorer before retrying.",
      );
    }
    const record = { hash, block: receipt.block_number };
    this.wallet.completed[action] = record;
    delete this.wallet.pending;
    await saveWallet(this.wallet);
    return record;
  }
  private async submit(
    action: Action,
    send: () => Promise<{ transaction_hash: string }>,
    operation: Operation,
  ): Promise<{ hash: string; block: number }> {
    if (this.wallet.pending)
      throw new Error("A transaction is already pending.");
    this.wallet.pending = { action };
    let sent: { transaction_hash: string };
    try {
      await saveWallet(this.wallet);
      this.submission = operation;
      sent = await send();
      // The signer persisted the SDK-computed hash before signing/broadcast.
      // A lost RPC response therefore remains recoverable via receipt lookup.
      if (
        !this.wallet.pending.hash ||
        BigInt(this.wallet.pending.hash) !== BigInt(sent.transaction_hash)
      )
        throw new Error(
          "The RPC returned a different transaction hash. Check the saved transaction in the explorer.",
        );
    } catch (error) {
      if (!this.wallet.pending.hash) {
        delete this.wallet.pending; // failed before any signature could be sent
        await saveWallet(this.wallet);
      }
      throw error;
    } finally {
      this.submission = undefined;
    }
    return this.operations.run(
      "Confirm transaction",
      () => this.receipt(action, sent.transaction_hash),
      operation,
    );
  }
  async deploy(): Promise<{ hash: string; block: number }> {
    await this.verifyNetwork();
    return this.operations.run("Deploy demo wallet", async (operation) => {
      const payload = {
        classHash: this.wallet.classHash,
        constructorCalldata: CallData.compile({
          publicKey: this.wallet.publicKey,
        }),
        addressSalt: this.wallet.publicKey,
        contractAddress: this.wallet.address,
      };
      const estimate = await this.operations.run(
        "Estimate fee",
        () =>
          this.account.estimateAccountDeployFee(payload, {
            tip: 0n,
            skipValidate: true,
          }),
        operation,
      );
      const max = ceiling(estimate.resourceBounds);
      if ((await this.balance()) < max)
        throw new Error(
          `Fund the wallet with at least ${decimal(max)} STRK for deployment.`,
        );
      return this.submit(
        "deploy",
        () =>
          this.account.deployAccount(payload, {
            tip: 0n,
            resourceBounds: estimate.resourceBounds,
          }),
        operation,
      );
    });
  }
  async execute(
    action: "shield" | "transfer" | "withdraw",
    amount: bigint,
    recipient: string,
  ): Promise<{ hash: string; block: number }> {
    await this.verifyNetwork();
    return this.operations.run(
      {
        shield: "Shield STRK",
        transfer: "Transfer privately",
        withdraw: "Withdraw STRK",
      }[action],
      async (operation) => {
        const fee = await this.poolFee();
        const allowance = fee + (action === "shield" ? amount : 0n);
        if ((await this.balance()) <= allowance)
          throw new Error(
            `Fund more public STRK: this action needs ${decimal(allowance)} STRK plus gas.`,
          );
        const proveAt = await this.operations.run(
          "Wait for spendable notes",
          async () => {
            if (action === "shield")
              return (await this.rpc.getBlockNumber()) - PROOF_DEPTH;
            return waitForBlock(networkConfig(this.wallet.network).ws,
              () => this.rpc.getBlockNumber(), async (head) => {
              const found = await this.transfers.discoverNotes();
              const notes = found.notes.get(BigInt(STRK)) ?? [];
              const required = notes.reduce(
                (b, n) =>
                  Math.max(
                    b,
                    Number(n.created ?? Number.MAX_SAFE_INTEGER) + 10,
                  ),
                0,
              );
              const block = head - PROOF_DEPTH;
              if (notes.length && block >= required) return block;
              return undefined;
            });
          },
          operation,
        );
        const transfers = this.makeTransfers(this.discovery.atBlock(proveAt));
        const builder = transfers
          .build({
            autoRegister: true,
            autoSetup: true,
            autoDiscover: { notes: "refresh", channels: "refresh" },
            autoSelectNotes: "naive",
          })
          .surplusTo(this.wallet.address)
          .with(STRK, (t) => {
            if (action === "shield") t.deposit({ amount });
            if (action === "transfer") t.transfer({ amount, recipient });
            if (action === "withdraw") t.withdraw({ amount, recipient });
          });
        const invocation = await this.operations.run(
          "Build private action",
          () => builder.createProofInvocation({ provingBlockId: proveAt }),
          operation,
        );
        const { callAndProof } = await this.operations.run(
          "Generate transaction proof",
          () => transfers.executeWithInvocation(invocation, proveAt),
          operation,
        );
        if (action === "shield") {
          const [screener] = await this.rpc.callContract(
            {
              contractAddress: this.wallet.pool,
              entrypoint: "get_screener_public_key",
              calldata: [],
            },
            "latest",
          );
          if (
            BigInt(screener!) !== 0n &&
            !callAndProof.proof.additionalData?.signature
          )
            throw new Error("Deposit screening did not return an attestation.");
        }
        const details: UniversalDetails = callAndProof.proof.proofFacts?.length
          ? {
              proofFacts: callAndProof.proof.proofFacts,
              proof: callAndProof.proof.data,
            }
          : {};
        const calls: Call[] = [
          {
            contractAddress: STRK,
            entrypoint: "approve",
            calldata: [
              this.wallet.pool,
              ...Object.values(cairo.uint256(allowance)).map(String),
            ],
          },
          callAndProof.call,
        ];
        const estimate = await this.operations.run(
          "Estimate fee",
          () => this.account.estimateInvokeFee(calls, { tip: 0n, ...details }),
          operation,
        );
        const required = ceiling(estimate.resourceBounds) + allowance;
        if ((await this.balance()) < required)
          throw new Error(
            `Public balance must cover up to ${decimal(required)} STRK, including fees and deposit.`,
          );
        return this.submit(
          action,
          () =>
            this.account.execute(calls, {
              tip: 0n,
              ...details,
              resourceBounds: estimate.resourceBounds,
            }),
          operation,
        );
      },
    );
  }
}
function ceiling(bounds: unknown): bigint {
  return Object.values(
    bounds as Record<
      string,
      { max_amount: string; max_price_per_unit: string }
    >,
  ).reduce(
    (total, r) => total + BigInt(r.max_amount) * BigInt(r.max_price_per_unit),
    0n,
  );
}
function decimal(value: bigint): string {
  return `${value / 10n ** 18n}.${(value % 10n ** 18n).toString().padStart(18, "0").slice(0, 5)}`;
}
