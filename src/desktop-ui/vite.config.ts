import tailwindcss from "@tailwindcss/vite";
import react from "@vitejs/plugin-react";
import path from "path";
import { defineConfig } from "vite";

export default defineConfig({
  plugins: [react(), tailwindcss()],
  resolve: {
    alias: [
      { find: "@va/ui/button", replacement: path.resolve(__dirname, "../shared/ui/src/button.tsx") },
      { find: "@va/ui/dropdown-menu", replacement: path.resolve(__dirname, "../shared/ui/src/dropdown-menu.tsx") },
      { find: "@va/ui/input", replacement: path.resolve(__dirname, "../shared/ui/src/input.tsx") },
      { find: "@va/ui", replacement: path.resolve(__dirname, "../shared/ui/src/index.ts") },
      { find: "@va/client", replacement: path.resolve(__dirname, "../shared/client-ts/src/index.ts") },
      { find: "@", replacement: path.resolve(__dirname, "./src") },
    ],
  },
  clearScreen: false,
  server: {
    port: 5181,
    strictPort: true,
    proxy: {
      "/tray": {
        target: "http://localhost:5182",
        rewrite: (p) => p.replace(/^\/tray/, ""),
      },
    },
  },
});
