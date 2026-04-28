import tailwindcss from "@tailwindcss/vite";
import react from "@vitejs/plugin-react";
import path from "path";
import { defineConfig } from "vite";

export default defineConfig({
  base: "/va/",
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
  server: {
    port: 5180,
    strictPort: true,
  },
});
