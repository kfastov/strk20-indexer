# Demo workflow

The [hosted demo](https://strk20.nullref.cc/demo/) and
[`ts/demo`](../../ts/demo) run a real software-wallet flow on Mainnet or Sepolia.
The default build uses public feeds and RPCs, with the local provider supplying
discovery results to the official transaction builder.

## Run locally

Follow the root README's [source build](../../README.md#build-from-source), then:

```sh
npm --prefix ts run dev
```

Endpoint configuration and static hosting are covered in
[Hosting](../ops/hosting.md#demo-and-sdk-builds). The page has no RPC settings form.

## Wallet and transaction cycle

1. Select the network and wait for discovery initialization. First startup
   downloads and verifies pool state before readiness; a valid local cache
   restores directly. The page shows actual stages and downloaded-file counts.
2. Create a wallet and export its backup. Signing and viewing keys are generated
   locally and saved per network in a wallet IndexedDB database, separate from
   the disposable discovery cache. Import the backup to recover that wallet.
3. Fund the displayed address with STRK, including public funds for gas and pool
   fees, then deploy the account. Funding detection reads the public token
   balance; it is not private-note discovery. The transaction path estimates fees
   and checks public balance before submission.
4. Choose **Deposit STRK** to shield an amount. Registration is included when
   required. Then choose **Discover transaction** to verify pool state and find
   the resulting note locally.
5. Choose **Transfer privately**. Self-transfer demonstrates spending the found
   note and creating a new one; another recipient must be registered. The builder
   uses local witnesses, channels and requirement checks pinned to one proving
   block. Wait for maturity when needed, then discover the result.
6. Choose **Withdraw STRK** to return shielded value to a public address, then
   discover again to update spent notes and private balance.

The primary action suggests the next step, and its menu allows repeated deposits,
transfers, withdrawals or discovery. Completed actions do not lock the wallet into
a one-time walkthrough. A pending transaction takes precedence and must be resumed
before another send. Wallet records and pending hashes survive page reloads.

The activity log shows requested operations and their transaction stages. Background
synchronization and cache work do not create extra user-operation entries. Timing
exports describe the selected operation; cold initialization, cached restoration,
proof generation and confirmation are different work and should not be compared
as interchangeable measurements.

## Recovering an already submitted transaction

If submission produced a hash but confirmation failed, use **Resume pending
transaction**. Do not repeat the original action as a new send. The demo saves
pending submission information and checks the existing hash after reload.

Confirmation uses transaction-status subscriptions with receipt checks. A terminal
subscription error, exhausted reconnects or timeout triggers one final bounded
receipt catch-up. It checks the hash, accepted block and execution status. If the
receipt succeeds, the demo records completion and clears the pending entry. If
confirmation is still unavailable, pending state remains resumable. Resume does
not sign or broadcast the transaction again. After confirmation, discover the
transaction to update local note state.

The relevant implementations are [submission recording](../../ts/demo/src/submission-signer.ts),
[confirmation](../../ts/demo/src/confirmation.ts) and
[transaction orchestration](../../ts/demo/src/transactions.ts).

## Privacy and limitations

- Local discovery keeps the viewing key in the wallet/Worker. Feed hosts still
  see network metadata such as IP addresses and request timing.
- **Official comparison is opt-in and discloses this wallet's viewing key to the
  official discovery service.** It observes the same block with both providers
  after one transaction; it does not send a second transaction. The local cache
  and the official request's cursor conditions differ. Agreement must include
  notes, amounts and spend witnesses before interpreting timing comparisons.
- **Hosted proving receives proving inputs.** Keeping discovery local does not
  make transaction construction private from the configured prover. Availability
  and any required screening attestation depend on that service.
- This is a browser software wallet. Keep its backup and use small amounts.
  Clearing the discovery cache preserves wallet data; deleting the wallet's
  browser storage without a backup can lose recovery material.
- Cold verification can be substantial, and all synchronous engine work shares
  one Worker. Neither a fixed startup bound nor a general speed advantage over
  official discovery is established.
- Verification covers state at a selected checkpoint, trusts local cache storage
  and the configured header RPC, and does not establish every historical
  transition or Ethereum finality. Conservative note maturity and a snapshot's
  history floor can delay spending. See [Consumer path](consumer-path.md).

## Historical transaction examples

These are the selected complete cycles from September 6–7, 2026. They illustrate
previous on-chain use of the local provider; they are not fresh acceptance runs or
performance results for the current build. The explorer links show transactions;
their receipts alone cannot prove which discovery provider an application used.

| Action | Network | Transaction | What the example demonstrates |
|---|---|---|---|
| Shield | Sepolia | [Transaction](https://sepolia.voyager.online/tx/0x760b6fc6e6cefa318a53071823466163b4ba48a2aac513ccbc3a7fc5cf0e88d) | Deposit into the pool before local note discovery. |
| Private transfer | Sepolia | [Transaction](https://sepolia.voyager.online/tx/0x48557def3325c9fc153b0e53e22fc624a51a65148d0f904b10a0d0099ed5386) | Self-transfer spends a discovered note and creates its replacement. |
| Withdrawal | Sepolia | [Transaction](https://sepolia.voyager.online/tx/0x28d8c41ea12d5ce715e557c69945f87a0e0260f4a0210bb0d6d20e2fd845ce4) | Return to public balance completes the shield–spend–withdraw cycle. |
| Shield | Mainnet | [Transaction](https://voyager.online/tx/0x3e09c1a8a09bf2a146bbd452fed3c48309b7124c7be4745056742ec1653bc59) | Mainnet deposit used for subsequent local discovery and spending. |
| Private transfer | Mainnet | [Transaction](https://voyager.online/tx/0x1df98fdd1cc335d5bfa9c39b4bcc55ae4cbc02a3ce389e14e3a6dcec2d8b5a3) | Private spending with the local provider supplying builder discovery data. |
| Withdrawal | Mainnet | [Transaction](https://voyager.online/tx/0x69ea91064cb35315d311493cac956439bf3e0bbe798c95248c7cc437802ef1f) | Completed withdrawal whose saved receipt was recovered after a confirmation failure, without resending. |
