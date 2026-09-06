import {
  AddressMap,
  Channel,
  Witness,
  type DiscoveryProviderInterface,
  type Note,
  type ViewingKey,
} from "@starkware-libs/starknet-privacy-sdk";
import { KeylessClient, type ClientOptions } from "./client.ts";
import type { DiscoveryResult } from "./types.ts";

type NotesParams = Parameters<DiscoveryProviderInterface["discoverNotes"]>[2];
type ChannelParams = Parameters<
  DiscoveryProviderInterface["discoverChannels"]
>[3];
type Recipients = Parameters<DiscoveryProviderInterface["discoverChannels"]>[2];
type NotesResult = Awaited<
  ReturnType<DiscoveryProviderInterface["discoverNotes"]>
>;
type ChannelsResult = Awaited<
  ReturnType<DiscoveryProviderInterface["discoverChannels"]>
>;
export interface AccountDiscovery {
  discoverNotes(params?: NotesParams): Promise<NotesResult>;
  discoverChannels(
    recipients?: Recipients,
    params?: ChannelParams,
  ): Promise<ChannelsResult>;
  discoverRequirement(
    recipient: bigint,
    token: bigint,
  ): ReturnType<DiscoveryProviderInterface["discoverRequirement"]>;
  restore(): Promise<NotesResult | undefined>;
}

/** Official interface plus account-bound convenience methods; one engine. */
export class LocalDiscoveryProvider implements DiscoveryProviderInterface {
  readonly client: KeylessClient;
  constructor(options: ClientOptions) {
    this.client = new KeylessClient(options);
  }
  get ready() {
    return this.client.ready;
  }
  forAccount(account: {
    address: bigint | string;
    viewingKey: ViewingKey;
  }): AccountDiscovery {
    const address = BigInt(account.address),
      key = account.viewingKey;
    return {
      discoverNotes: (params?: NotesParams) =>
        this.discoverNotes(address, key, params),
      discoverChannels: (
        recipients: Recipients = "all",
        params?: ChannelParams,
      ) => this.discoverChannels(address, key, recipients, params),
      discoverRequirement: (recipient: bigint, token: bigint) =>
        this.discoverRequirement(address, key, recipient, token),
      restore: async () => {
        const data = await this.client.discover(
          address,
          BigInt(key),
          undefined,
          true,
        );
        return data ? notesResult(data) : undefined;
      },
    };
  }
  async discoverNotes(
    address: bigint,
    key: ViewingKey,
    params?: NotesParams,
  ): Promise<NotesResult> {
    const data = await this.client.discover(
      address,
      BigInt(key),
      blockNumber(params?.blockIdentifier),
    );
    if (!data) throw new Error("Discovery state unavailable");
    return notesResult(data, params?.tokens);
  }
  async discoverChannels(
    address: bigint,
    key: ViewingKey,
    recipients: Recipients,
    params?: ChannelParams,
  ): Promise<ChannelsResult> {
    const result = await this.client.channels(
      address,
      BigInt(key),
      Array.isArray(recipients) ? recipients : null,
      blockNumber(params?.blockIdentifier),
    );
    if (recipients === "total-only")
      return { timestamp: result.block, total: result.total };
    const channels = new AddressMap<Channel>();
    for (const channel of result.channels) {
      channels.set(
        BigInt(channel.recipient),
        new Channel(
          BigInt(channel.publicKey),
          channel.key ? BigInt(channel.key) : undefined,
          channel.tokens.map((t) => [
            BigInt(t.token),
            { tokenIndex: t.tokenIndex, noteNonce: t.noteNonce },
          ]),
        ),
      );
    }
    return { timestamp: result.block, channels, total: result.total };
  }
  async discoverRequirement(
    address: bigint,
    key: ViewingKey,
    recipient: bigint,
    token: bigint,
    block?: number,
  ) {
    return this.client.requirement(
      address,
      BigInt(key),
      recipient,
      token,
      block,
    );
  }
  /** Pin all builder reads, including requirement(), to one proving block. */
  atBlock(block: number): DiscoveryProviderInterface {
    return {
      discoverNotes: (address, key, params) =>
        this.discoverNotes(address, key, { ...params, blockIdentifier: block }),
      discoverChannels: (address, key, recipients, params) =>
        this.discoverChannels(address, key, recipients, {
          ...params,
          blockIdentifier: block,
        }),
      discoverRequirement: (address, key, recipient, token) =>
        this.discoverRequirement(address, key, recipient, token, block),
    };
  }
  waitForBlock(block: number) {
    return this.client.waitForHead(block);
  }
  subscribe() {
    return this.client.subscribe();
  }
  close() {
    return this.client.close();
  }
}
function blockNumber(block: unknown): number | undefined {
  if (block === undefined || block === "latest") return undefined;
  if (typeof block === "number" && Number.isSafeInteger(block) && block >= 0)
    return block;
  if (typeof block === "object" && block !== null && "block_number" in block)
    return blockNumber(block.block_number);
  throw new Error("BOUND_UNAVAILABLE: use an explicit block number or latest");
}
export function notesResult(
  data: DiscoveryResult,
  tokens?: bigint[],
): NotesResult {
  if (!data.report.incoming_complete || !data.report.outgoing_complete)
    throw new Error("DISCOVERY_INCOMPLETE");
  const notes = new AddressMap<Note[]>();
  for (const n of data.notes) {
    const token = BigInt(n.token);
    if (n.spent || (tokens && !tokens.includes(token))) continue;
    const list = notes.get(token) ?? [];
    list.push({
      id: BigInt(n.id),
      amount: BigInt(n.amount),
      open: n.witness.salt === "1",
      created: n.knownByBlock,
      sender: BigInt(n.sender),
      witness: new Witness(
        BigInt(n.witness.channelKey),
        n.witness.index,
        BigInt(n.witness.salt),
      ),
    });
    notes.set(token, list);
  }
  const incomingChannels = new AddressMap<
    Parameters<NotesResult["cursor"]["incomingChannels"]["set"]>[1]
  >();
  for (const [sender, ch] of Object.entries(
    data.incomingCursor.channels ?? {},
  )) {
    const noteIndexes = new AddressMap<number>(),
      totalNoteCounts = new AddressMap<number>();
    for (const [token, sub] of Object.entries(ch.subchannels ?? {})) {
      noteIndexes.set(BigInt(token), (sub.last_note_index ?? -1) + 1);
      totalNoteCounts.set(BigInt(token), (sub.last_note_index ?? -1) + 1);
    }
    incomingChannels.set(BigInt(sender), {
      channelKey: BigInt(ch.channel_key),
      subchannelIdIndex: (ch.last_subchannel_index ?? -1) + 1,
      noteIndexes,
      totalNoteCounts,
    });
  }
  return {
    timestamp: data.block,
    notes,
    cursor: { blockId: data.block, incomingChannels },
  };
}
