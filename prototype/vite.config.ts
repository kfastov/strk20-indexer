import { defineConfig } from 'vite';

// Deliberately bare. The prototype has no dependencies to pre-bundle and no
// framework plugin; keeping this file empty of cleverness is part of the point.
export default defineConfig({
  server: { port: 5180, strictPort: false, open: false },
  build: { target: 'es2022', outDir: 'dist' },
});
