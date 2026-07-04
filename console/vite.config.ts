/// <reference types="vitest/config" />
import { defineConfig } from 'vitest/config'
import react from '@vitejs/plugin-react'
import tailwindcss from '@tailwindcss/vite'

// The console is a static SPA served by the appliance (D1) and the shadow-mode
// artifact (D2). API base URLs are injected via VITE_WSF_API_BASE /
// VITE_AOG_BASE at build time, with localhost defaults for `npm run dev`.
export default defineConfig({
  plugins: [react(), tailwindcss()],
  server: {
    port: 5173,
    host: true
  },
  test: {
    environment: 'jsdom',
    setupFiles: ['./src/test/setup.ts'],
    css: true,
    include: ['src/**/*.test.{ts,tsx}']
  }
})
