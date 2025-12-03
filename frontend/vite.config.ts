import { defineConfig } from 'vite'
import { svelte } from '@sveltejs/vite-plugin-svelte'

// https://vite.dev/config/
export default defineConfig({
  plugins: [svelte()],
  build: {
    outDir: '../wwwroot',
    emptyOutDir: true,
    minify: 'terser',
    terserOptions: {
      compress: {
        drop_console: true,
        drop_debugger: true,
        ecma: 2020,
        module: true,
        toplevel: true
      },
      mangle: {
        properties: {
          regex: /^__/,
        }
      },
      format: {
        comments: false
      }
    },
    rollupOptions: {
      output: {
        entryFileNames: 'assets/[name].[hash].js',
        chunkFileNames: 'assets/[name].[hash].js',
        assetFileNames: 'assets/[name].[hash].[ext]',
        manualChunks: (id) => {
          // Consolidate all node_modules into a single vendor chunk
          if (id.includes('node_modules')) {
            return 'vendor';
          }
        }
      }
    },
    cssMinify: true,
    reportCompressedSize: false,
    chunkSizeWarningLimit: 1000,
    sourcemap: false, // Disable sourcemaps for production build
    modulePreload: {
      polyfill: false // Disable module preload polyfill as it's supported in modern browsers
    },
    target: 'es2020' // Target modern browsers for smaller bundle size
  },
  server: {
    proxy: {
      '/api': {
        target: 'http://localhost:5025',
        changeOrigin: true
      }
    }
  },
  base: '/',
  // Optimize loading
  optimizeDeps: {
    include: ['svelte', 'svelte/animate', 'svelte/easing', 'svelte/motion', 'svelte/transition', 'svelte/internal']
  }
})
