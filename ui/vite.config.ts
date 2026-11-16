import { defineConfig } from "vite";
import solid from "vite-plugin-solid";

export default defineConfig({
  plugins: [solid()],
  clearScreen: false,
  server: { strictPort: true, port: 5173 },
  build: { target: "esnext" },
});
