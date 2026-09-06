import { test } from "node:test";
import assert from "node:assert/strict";
import { createServer } from "node:http";
import {
  readFile,
  writeFile,
  mkdtemp,
  readdir,
  stat,
  rm,
} from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { once } from "node:events";
import { NodeDiscoveryProvider } from "../dist/node.js";

const fixture = (path: string) =>
  readFile(new URL(`../../../crates/wasm/fixture/${path}`, import.meta.url));
const json = async (path: string) =>
  JSON.parse(await fixture(path).then((bytes) => bytes.toString()));

test("Node worker verifies real state and restores the account with the server offline", async () => {
  const genesis = await json("genesis.json"),
    checkpoint = await json("checkpoint.json");
  const owner = (await json("owners.json"))[0];
  const cacheDirectory = await mkdtemp(join(tmpdir(), "strk20-node-test-"));
  const failures: unknown[] = [],
    requests: string[] = [];
  const server = createServer(async (request, response) => {
    try {
      let body = "";
      for await (const part of request) body += part;
      requests.push(`${request.url}:${body}`);
      if (request.method === "POST") {
        const call = JSON.parse(body);
        const result =
          call.method === "starknet_chainId"
            ? `0x${Buffer.from(genesis.chain_id).toString("hex")}`
            : call.method === "starknet_getBlockWithTxHashes"
              ? {
                  block_number: 99,
                  block_hash: checkpoint.block_hash,
                  new_root: checkpoint.state_root,
                  status: "ACCEPTED_ON_L2",
                }
              : call.method === "starknet_getStorageProof"
                ? await json("proof.json")
                : undefined;
        assert.notEqual(
          result,
          undefined,
          "only public checkpoint methods are used",
        );
        response.end(JSON.stringify({ jsonrpc: "2.0", id: call.id, result }));
      } else {
        const files: Record<string, string> = {
          "/genesis.json": "genesis.json",
          "/manifest.json": "manifest.json",
          "/head.ndjson": "head.ndjson",
          "/snapshots/00000000.strk20s.zst": "snapshots/0.zst",
        };
        assert(files[request.url!], `unexpected path ${request.url}`);
        response.end(await fixture(files[request.url!]!));
      }
    } catch (error) {
      failures.push(error);
      response.writeHead(500).end();
    }
  });
  server.listen(0, "127.0.0.1");
  await once(server, "listening");
  const address = server.address();
  assert(address && typeof address === "object");
  const endpoint = `http://127.0.0.1:${address.port}`;
  const options = {
    cacheDirectory,
    feedUrl: endpoint,
    rpcUrl: endpoint,
    proofRpcUrl: endpoint,
    network: {
      name: "fixture",
      chainId: genesis.chain_id,
      pool: genesis.pool,
      genesisBlock: 0,
      epochSize: 100,
      feedFormat: 1,
    },
  };
  let provider = new NodeDiscoveryProvider(options);
  try {
    const account = {
      address: owner.owner,
      viewingKey: BigInt(`0x${owner.key}`),
    };
    const found = await provider
      .forAccount(account)
      .discoverNotes({ blockIdentifier: 99 });
    assert([...found.notes.values()].flat().length > 0);
    await provider.close();
    await new Promise<void>((resolve, reject) =>
      server.close((error) => (error ? reject(error) : resolve())),
    );
    assert.deepEqual(failures, []);
    assert(requests.every((request) => !request.includes(owner.key)));
    const files = await readdir(cacheDirectory);
    assert.equal(files.length, 1);
    assert.equal(
      (await stat(join(cacheDirectory, files[0]!))).mode & 0o777,
      0o600,
    );
    provider = new NodeDiscoveryProvider(options);
    assert.equal((await provider.ready).verifiedAt, 99);
    const restored = await provider.forAccount(account).restore();
    assert(restored);
    assert.equal(restored.timestamp, found.timestamp);
    assert.deepEqual([...restored.notes], [...found.notes]);
    const cursors = (value: typeof found) =>
      [...value.cursor.incomingChannels].map(([sender, cursor]) => ({
        sender,
        channelKey: cursor.channelKey,
        subchannelIdIndex: cursor.subchannelIdIndex,
        noteIndexes: [...cursor.noteIndexes],
        totalNoteCounts: [...cursor.totalNoteCounts],
      }));
    assert.deepEqual(cursors(restored), cursors(found));
  } finally {
    await provider.close();
    server.close();
    await rm(cacheDirectory, { recursive: true, force: true });
  }
});

test("failed cache initialization still closes the worker cleanly", async () => {
  const directory = await mkdtemp(join(tmpdir(), "strk20-node-failure-"));
  await writeFile(join(directory, "file"), "not a directory");
  const provider = new NodeDiscoveryProvider({
    network: "mainnet",
    feedUrl: "https://unused.invalid",
    cacheDirectory: join(directory, "file", "cache"),
  });
  try {
    await assert.rejects(provider.ready, /ENOTDIR/);
    await provider.close();
  } finally {
    await provider.close();
    await rm(directory, { recursive: true, force: true });
  }
});
