import { createReadStream, existsSync, statSync } from 'node:fs';
import { join, normalize } from 'node:path';
import { fileURLToPath } from 'node:url';
import { defineConfig, type Plugin } from 'vite';

const here = fileURLToPath(new URL('.', import.meta.url));
const MAINNET_FEED = join(here, '..', '..', 'data', 'mainnet', 'feed');

/**
 * Serves this repository's REAL mainnet feed at /mainnet-feed, so the demo has
 * a lane whose bytes, hashes, epoch count and sizes are all real rather than
 * generated. Read-only, path-normalised, and scoped to that one directory.
 *
 * `Timing-Allow-Origin` is set because demo-app.md §9 wants
 * PerformanceResourceTiming.transferSize to be available — otherwise the panel
 * would have to print a wrong 0, and it prints `n/a` instead.
 */
function mainnetFeed(): Plugin {
  return {
    name: 'strk20-mainnet-feed',
    configureServer(server) {
      server.middlewares.use('/mainnet-feed', (req, res, next) => {
        const rel = normalize(decodeURIComponent((req.url ?? '/').split('?')[0]!)).replace(/^(\.\.[/\\])+/, '');
        const file = join(MAINNET_FEED, rel);
        if (!file.startsWith(MAINNET_FEED) || !existsSync(file) || !statSync(file).isFile()) return next();
        res.setHeader('Timing-Allow-Origin', '*');
        res.setHeader(
          'Content-Type',
          file.endsWith('.json') ? 'application/json' : file.endsWith('.ndjson') ? 'application/x-ndjson' : 'application/octet-stream',
        );
        createReadStream(file).pipe(res);
      });
    },
  };
}

export default defineConfig({
  plugins: [mainnetFeed()],
  server: { port: 5190, strictPort: false, open: false, headers: { 'Timing-Allow-Origin': '*' } },
  build: { target: 'es2022', outDir: 'dist' },
});
