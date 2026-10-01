import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';
import tailwindcss from '@tailwindcss/vite';
import { fileURLToPath } from 'node:url';

export default defineConfig({
  plugins: [react(), tailwindcss()],
  resolve: {
    alias: {
      '@': fileURLToPath(new URL('./src', import.meta.url)),
    },
  },
  server: {
    // Bind to IPv4 explicitly: on Windows, Node resolves `localhost` to `::1`,
    // which leaves the dev server unreachable from the WebView.
    host: '127.0.0.1',
    port: 5173,
    strictPort: true,
  },
});
