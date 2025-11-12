import { defineConfig } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";

// https://vite.dev/config/
export default defineConfig({
  plugins: [svelte()],
  
  // Vite options tailored for Tauri development
  clearScreen: false,
  
  server: {
    port: 5173,
    strictPort: true,
    proxy: {
      '/api': {
        target: 'http://localhost:5333',
        changeOrigin: true
      }
    },
    watch: {
      ignored: ["**/src-tauri/**"],
    },
  },
  
  build: {
    outDir: 'dist',
    emptyOutDir: true,
    minify: 'esbuild',
    rollupOptions: {
      output: {
        entryFileNames: 'assets/[name].[hash].js',
        chunkFileNames: 'assets/[name].[hash].js',
        assetFileNames: 'assets/[name].[hash].[ext]',
      }
    },
    chunkSizeWarningLimit: 1000
  },
  
  base: '/'
});
