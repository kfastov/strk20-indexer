import { installWorker } from "./worker.ts";
import init, { Engine } from "./wasm/strk20_engine.js";
installWorker(async () => {
  await init();
  return { Engine };
});
