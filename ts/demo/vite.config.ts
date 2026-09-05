import { defineConfig } from "vite";

export default defineConfig(({ command, isPreview }) => ({
  base:
    process.env.DEMO_BASE ??
    (command === "serve" && !isPreview ? "/" : "/demo/"),
  server: { port: 5190, open: false },
  worker: { format: "es" },
  build: { target: "es2022", outDir: "dist" },
}));
